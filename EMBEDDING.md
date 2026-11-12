# Embedding revolt-rs in another Rust PHP extension

An extension written in Rust (a database driver, say) can embed revolt-rs as a library. Then:

- installing only that extension gives PHP the native `Revolt\EventLoop` classes too (no
  revolt/event-loop package, no `revolt` extension needed);
- the extension drives the loop from Rust — fd watchers, timers, defers, suspensions — without
  any PHP-level call, whichever copy of revolt-rs registered the classes;
- the extension's own sockets can live in the loop's poller as *sources*: registered
  edge-triggered once, their readiness handed to the extension in one batch per poll (no Revolt
  callback per socket, no second poller);
- the loop's driver is always native: `EventLoop::setDriver()` is refused (DIVERGENCES.md), so
  PHP code and every embedder share that one loop;
- when the classes are not revolt-rs's (the PHP library, if the embedder chose not to register
  them) or the owning copy is incompatible, the embedder may fall back to calling
  `Revolt\EventLoop` through PHP.

[examples/embedder](examples/embedder/src/lib.rs) is a complete embedder (the extension
`revolt_embed_example`) and [tests/embedder](tests/embedder/EmbedderTest.php) proves each case
above; run them with `tools/embed-test.sh`.

## Several copies, one loop

Each embedder carries its own copy of revolt-rs, maybe of another version, and the standalone
`revolt` extension may be loaded as well. At MINIT, the first copy that finds no
`Revolt\EventLoop` class registers all the classes and owns the loop (the "owner"); every later
copy registers nothing. There is one loop per PHP thread, the owner's, and every copy uses it:

- each copy exports a C-ABI function table, the symbol `revolt_rs_abi_v1` (`revolt_rs::embed::Abi`);
- a copy that is not the owner finds the owner's table from the address of the internal handler
  of `Revolt\EventLoop::getDriver` (`dladdr` + `dlsym` on Unix, `GetModuleHandleExW` +
  `GetProcAddress` on Windows) and uses it if its `abi_major` equals its own and its `size` is
  at least its own (`revolt_rs::embed::active()`);
- otherwise (another major, an older smaller table, or `Revolt\EventLoop` is a PHP class) there
  is no native path: `NativeLoop::current()` returns `None` and the embedder uses the PHP-call
  fallback.

Within one major version the table only grows (new functions are appended, `abi_minor` goes up),
so a newer copy can use an older owner's table only when it is large enough; drivers and
suspensions cross the boundary as `zend_object` pointers, and everything runs inside the owner.

| `abi_minor` | Added |
|---|---|
| 0 | the callbacks, suspensions, `hold`/`unhold` |
| 1 | the sources (`src_group`, `src_close`, `src_reference`, `src_arm`, `src_disarm`, `src_forget`); `current` never returns a non-native driver (`setDriver()` is refused) |

A copy of minor 1 or later finds an owner of minor 0 incompatible (its table is smaller): it
must use the fallback, or do without the loop (the mongodb extension then blocks).

Which extension owns the classes depends on the load order only; nothing else changes.
`ReflectionClass::getExtensionName()` of `Revolt\EventLoop` names it.

## Cargo and build setup

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
revolt-rs = { git = "https://github.com/danog/revolt-rs", tag = "..." }  # the crate crates/revolt-rs
```

- revolt-rs pins `ext-php-rs = "=0.16.4"` with the features `runtime` and `observer`; an
  embedder using ext-php-rs itself must use the same version (Cargo unifies the features).
- ext-php-rs generates its Zend bindings from an allowlist: add the symbols revolt-rs needs, the
  list in [crates/revolt-rs/BINDINGS](crates/revolt-rs/BINDINGS), to `EXT_PHP_RS_ALLOWED_BINDINGS`
  (e.g. in `.cargo/config.toml`'s `[env]`, as [this repository](.cargo/config.toml) does), next to
  the embedder's own additions.
- `PHP` and `PHP_CONFIG` select the PHP to build for (8.2 to 8.6, NTS or ZTS, non-debug,
  64-bit); `revolt_rs::module::{ZEND_MODULE_API_NO, BUILD_ID}` are what the module entry needs.
  PHP 8.6 needs the `[patch.crates-io]` entry for ext-php-rs-build of this repository's
  Cargo.toml in the embedder's own workspace (see [patches/README.md](patches/README.md)): Cargo
  applies only the root workspace's patches.
- macOS: link with `-undefined dynamic_lookup`. Linux and other ELF systems: link with
  `-Wl,-Bsymbolic`, so that the copy's references to its own exported table never bind to
  another copy's (PHP loads extensions with `RTLD_GLOBAL`). Windows: nightly Rust and
  `#![feature(abi_vectorcall)]` (PHP's internal function handlers are `__vectorcall` there; the
  `revolt_rs::zh!` macro makes such a handler from an `extern "C"` one). See
  [examples/embedder/build.rs](examples/embedder/build.rs).

## Module hooks

Call revolt-rs's hooks from the extension's own (or build the whole entry with
`revolt_rs::module::module_entry` and set its `functions`, as the example does):

| Hook | Call |
|---|---|
| MINIT | `revolt_rs::module::minit(module_number)` → whether this copy owns the classes; `minit_with(module_number, false)` never registers them (use whatever implementation is there) |
| RINIT | `revolt_rs::module::rinit()` |
| RSHUTDOWN | `revolt_rs::module::rshutdown()` (the owner destroys the thread's driver here) |
| MSHUTDOWN | `revolt_rs::module::mshutdown()` |
| MINFO | `revolt_rs::module::minfo()` (optional: phpinfo() rows) |

## The native path

`revolt_rs::embed::NativeLoop::current()` returns the fast path for the calling thread's
current driver (creating the default driver if there is none), or `None` (use the fallback; an
exception may be pending if creating the driver failed). The driver is the same for the whole
request (`setDriver()` is refused); a new request has a new one.

| `NativeLoop` method | Revolt equivalent |
|---|---|
| `on_readable(fd, socket, cb, data, release, held)` / `on_writable(...)` | `onReadable()` / `onWritable()` on an OS handle: an fd, or a `SOCKET` on Windows with `socket = true` |
| `delay(secs, cb, data, release, held)` / `repeat(...)` | `delay()` / `repeat()` (0 with an exception for a negative interval) |
| `defer(cb, data, release, held)` | `defer()` |
| `queue(f, data)` | `queue()`: `f(data, 0)` runs once, before any other callback |
| `cancel(id)`, `enable(id)`, `disable(id)`, `reference(id)`, `unreference(id)` | the same (`enable`/`reference` return false for an invalid id, without an exception, or with one pending) |
| `suspension()` | `getSuspension()` of the running fiber or `{main}` (a new reference) |
| `suspend(s)`, `resume(s, value)`, `throw(s, exception)` | `Suspension::suspend()`, `resume()`, `throw()` |
| `hold(o)` / `unhold()` | keep an object alive for the running fiber, visible to the cycle collector (below) |

Callbacks are `unsafe extern "C" fn(data: *mut c_void, id: u64)`. Their ids are real Revolt
callbacks of the driver: `revolt_rs::core::id_to_string(id)` is the string id PHP code sees in
`getIdentifiers()`, `getType()`, `cancel()`, and so on. A callback may call back into the loop
(cancel itself, resume a suspension); an exception it leaves pending is handled as one thrown by
a PHP callback (the error handler, or `UncaughtThrowable` out of `run()`).

`release(data)` (optional) runs exactly once, when the callback is gone: after a defer/delay
fired, on `cancel()`, or when its driver is destroyed (at the latest at RSHUTDOWN). It must not
call into the loop for the same callback.

`held` (nullable) is an object the callback keeps alive: a new reference is taken, released
after `release`, and reported to PHP's cycle collector. Pass the suspension a callback resumes as
`held` and only borrow it in `data`: PHP closures capturing a suspension are visible to the
collector, references hidden in Rust data are not.

### Waiting

The pattern of [examples/embedder](examples/embedder/src/lib.rs) (`native_wait`): suspend the
running fiber (or `{main}`) until a native callback resumes it.

```rust
let nl = NativeLoop::current()?;           // None: use the PHP fallback
let s = nl.suspension();                   // a new reference
nl.hold(s);                                // kept alive where the cycle collector sees it...
release_object(s);                         // ...not by this frame
let id = nl.on_readable(fd, false, fire, data /* borrows s */, Some(release), s /* held */);
let value = nl.suspend(s);                 // None: an exception is pending (thrown in, the
nl.cancel(id);                             //   loop's uncaught error, or the fiber's destruction)
nl.unhold();
```

Why `hold`: while a fiber is suspended, a reference owned by a native frame is invisible to the
cycle collector, so a fiber suspended in such a wait whose last references form a cycle (with
the suspension, say) could never be collected, where Revolt's PHP code would be. Held objects are
reported by the fiber itself, as its local variables are; when the collector destroys the fiber,
`suspend()` returns with the unwind exception and the held objects are still there for the
`cancel()`. Holds are LIFO per fiber.

Raw fds bypass PHP's stream layer: data already in a PHP stream's read buffer is not seen (the
PHP-level `onReadable()` reports such a stream readable at once).

## Sources

An embedder that does its own non-blocking socket I/O (a database driver: its connections,
their reads and writes) can put those sockets in the driver's poller instead of making a Revolt
callback per wait. A wait then costs no control call and no extra system call: the loop's one
`epoll_wait()`/`kevent()` reports the socket, and a fiber or `{main}` waiting on it resumes.

```rust
let g = nl.src_group(take, flush, data);    // once per driver (one per request)
// a wait, after the socket's read or write returned EAGAIN:
let fds = [SrcFd { fd, events: SRC_READ as u32 }];
if nl.src_arm(g, &fds, token)? {            // readiness seen earlier: retry the I/O now
    return Ok(());
}
nl.src_reference(g, true);                  // while waits pend: the loop keeps running
let s = nl.suspension();                    // suspend as in "Waiting" (hold it, ...)
nl.suspend(s);                              // `flush` resumes it
nl.src_disarm(&fds, token);
// before closing the socket:
nl.src_forget(fd);
```

- A source is registered with the kernel at its first `src_arm` and stays registered until
  `src_forget` (call it before closing the fd: a new fd with the same number must be registered
  anew) or until the group or the driver goes. Readiness is edge-triggered (Linux: `EPOLLET`
  with `EPOLLIN|EPOLLOUT|EPOLLRDHUP`; kqueue: `EVFILT_READ` with `EV_CLEAR`, `EVFILT_WRITE`
  one-shot per wait that wants it; Windows: an armed source is polled with WSAPoll, level-
  triggered, only while armed). So arm a source only after its I/O returned EAGAIN (or right
  after sending a request, before its reply can be there).
- An event of a source that nobody waits for is kept as ready bits; the next `src_arm` of that
  source for what was seen returns `Ok(true)` at once (consumed, nothing armed): retry the I/O
  (it may find nothing: then arm again, which now waits).
- During the loop's poll, `take(data, token)` is called for each event of an armed source
  (`token`: what `src_arm` was given). It returns whether the waiter took it (false: it no
  longer waits, e.g. it timed out but has not disarmed yet; the readiness is then kept as ready
  bits). It must not call into the loop or PHP. A source is disarmed when its event is taken.
- After that poll, `flush(data)` is called once per group whose `take` accepted something: it
  resumes those waiters (`resume` of their suspensions: a fiber's resumes as a microtask, as
  Revolt resumes fibers; `{main}`'s ends the loop's run). It runs in the loop's own context, not
  in a callback fiber, so it must not suspend; it should call no PHP code.
- `src_reference(g, true)` keeps the loop running (the driver's `isEmpty()` is false, `run()`
  does not return) while the group has pending waits; unreference it when the last ends.
- Groups and sources are not Revolt callbacks: `getIdentifiers()`, `getInfo()` and
  `TracingDriver` do not see them.
- A socket is a source or watched by callbacks (`on_readable`...), not both on one driver.
- `fork()`: the child's poller is made anew; its sources are registered again at their next arm
  (armed ones at once).

## The PHP-call fallback

`revolt_rs::embed::php::call(lcname, args)` calls a static method of `Revolt\EventLoop` (the
class is autoloaded if needed, so the PHP library works too). The example's `php_wait` does what
PHP code would: `$s = EventLoop::getSuspension()`, `EventLoop::delay($t, $s->resume(...))` (or
`onReadable`), `$s->suspend()`, `EventLoop::cancel($id)`.

The fallback is taken when `NativeLoop::current()` is `None`:

- the classes are the PHP library's (the embedder used `minit_with(.., false)` or loaded after
  something that defined them; normally an embedder's MINIT registration keeps an installed
  revolt/event-loop package dormant);
- the owning copy's table is incompatible (another ABI major, or smaller than this copy's).

The loop's driver itself is always native: `EventLoop::setDriver()` is refused, so a
`TracingDriver`, a user `Driver` or a PHP subclass of `AbstractDriver` never runs the loop (they
still work when used directly).

## Composer

An embedder should declare what it brings, in its `composer.json`:

```json
"provide": {
    "ext-revolt": "*",
    "revolt/event-loop": "1.0.9"
}
```

with the revolt/event-loop version the embedded revolt-rs implements
(`revolt_rs::module::REVOLT_VERSION`), the same one danog/revolt-rs itself provides. `provide`
only affects projects that require the embedder through Composer: Composer then does not install
revolt/event-loop for them. Everywhere else, revolt/event-loop may well be installed; it stays
unused, because the classes registered at MINIT exist before Composer's autoloader would load
its files.
