# revolt-rs: plan

A PHP extension in pure Rust that natively implements the public API of
[revolt/event-loop](https://github.com/revoltphp/event-loop) **v1.0.9** (tag `v1.0.9`, commit
`44061cf513e53c6200372fc935ac42271566295d`). The extension registers the same classes at MINIT,
so Composer's autoloader never loads the PHP versions and existing code (Amp v3, anything on
Revolt) runs unchanged on the native loop.

Ships in two forms:

1. the standalone `revolt` extension (`revolt.so` / `php_revolt.dll`, composer package
   `danog/revolt-rs`, `type: php-ext`, `provide: {"revolt/event-loop": "1.0.9"}`);
2. embedded in other Rust extensions (first: danog/mongo-php-async-driver), through the
   `revolt-rs` library crate, so that installing only the embedding extension gives the native
   Revolt classes too and the embedder drives the loop from Rust without PHP-level calls.

## Workspace layout

```
Cargo.toml              workspace + the standalone extension package (cdylib, lib name `revolt`)
build.rs                macOS `-undefined dynamic_lookup` for the cdylib
src/lib.rs              the module entry of `revolt` (thin: calls revolt_rs::module hooks)
crates/revolt-rs/       library crate `revolt_rs`: the whole implementation
  build.rs              PHP detection (php-config/PHP), cfgs php83..php86 / php_zts, module API id
  src/zend/             Zend layer over ext-php-rs's raw bindings (zvals, strings, hashes,
                        class registration, calls, fibers, closures, weak refs, streams)
  src/api/              class/method registration of the whole Revolt API (arginfo tables)
  src/core/             the native loop: callback records, queues, ticks, run, suspensions,
                        error handling, timers (heap), the callback fiber
  src/backend/          the OS reactor: epoll (Linux), kqueue (macOS/BSD), WSAPoll (Windows),
                        signals (sigaction + wake pipe), fork detection (pthread_atfork)
  src/classes/          PHP-visible behaviour of each class (EventLoop, AbstractDriver, the
                        Driver\* classes, DriverSuspension, FiberLocal, errors, TimerQueue, ...)
  src/embed.rs          the embedding API (Rust side) + the versioned C ABI table
  src/module.rs         MINIT/RINIT/RSHUTDOWN/MSHUTDOWN/MINFO hooks for any host module
examples/embedder/      a second PHP extension (`revolt_embed_example`) embedding revolt-rs:
                        proves the two-copies-loaded case, the foreign-native fast path and the
                        PHP-level fallback (against the PHP library)
tests/revolt/           revolt/event-loop v1.0.9's test suite (MIT notice), unchanged
tests/native/           our own PHPUnit tests: divergences, robustness, embedding
tests/NOT_APPLICABLE.md the carried tests that cannot apply, each with a reason
tools/dump-api.php      Reflection dump of the Revolt API (PHP library vs extension)
tools/api-1.0.9.json    the dump of the PHP library (the reference)
tools/asan.sh           ASan build + suite run (Docker, Linux)
tools/bench/            benchmarks (native vs PHP StreamSelectDriver/EvDriver)
config.m4, Makefile.frag phpize/PIE build (cargo build)
.github/workflows/      tests.yml (Linux/macOS/Windows, PHP 8.2-8.6, NTS/ZTS), release.yml
```

## API inventory (revolt/event-loop v1.0.9)

| Class | Kind | Notes |
|---|---|---|
| `Revolt\EventLoop` | final class | static facade, private static `$driver`, private ctor |
| `Revolt\EventLoop\Driver` | interface | 23 methods incl. `__debugInfo` |
| `Revolt\EventLoop\Suspension` | interface | resume/suspend/throw |
| `Revolt\EventLoop\CallbackType` | pure enum | Defer, Delay, Repeat, Readable, Writable, Signal |
| `Revolt\EventLoop\DriverFactory` | final class | `create()`, private `createDriverFromEnv()` |
| `Revolt\EventLoop\FiberLocal` | final class | per-fiber storage (WeakMaps), `clear()` |
| `Revolt\EventLoop\InvalidCallbackError` | final, extends Error | consts E_NONNULL_RETURN=1, E_INVALID_IDENTIFIER=2, private ctor |
| `Revolt\EventLoop\UncaughtThrowable` | final, extends Error | private ctor, two factories |
| `Revolt\EventLoop\UnsupportedFeatureException` | final, extends Exception | |
| `Revolt\EventLoop\Driver\StreamSelectDriver` | final, extends AbstractDriver | **not in revolt-rs**: replaced by `Driver\NativeDriver` |
| `Revolt\EventLoop\Driver\EvDriver` / `EventDriver` / `UvDriver` | final, extend AbstractDriver | **not in revolt-rs** (see above) |
| `Revolt\EventLoop\Driver\TracingDriver` | final, implements Driver | wraps a Driver, `dump()` |
| `Revolt\EventLoop\Internal\AbstractDriver` | abstract, implements Driver | **extensible**: abstract activate/dispatch/deactivate/now/getHandle, final protected enqueueCallback/error, final __serialize/__unserialize |
| `Revolt\EventLoop\Internal\DriverCallback` | abstract | public props invokable/enabled/referenced, readonly id/closure, __get/__set throw |
| `Revolt\EventLoop\Internal\{Defer,Timer,Signal}Callback`, `StreamCallback` (abstract), `Stream{Readable,Writable}Callback` | final classes | readonly promoted props |
| `Revolt\EventLoop\Internal\DriverSuspension` | final, implements Suspension | |
| `Revolt\EventLoop\Internal\TimerQueue` | final | min-heap of TimerCallback by expiration |
| `Revolt\EventLoop\Internal\ClosureHelper` | final | `static getDescription(Closure)` |

No functions, no global constants. Parity is checked by `tools/dump-api.php` (classes, flags,
parents, interfaces, constants, properties with types/defaults/modifiers, methods with
modifiers/return types/parameters with names/types/defaults/variadic/by-ref, enum cases):
the dump of the extension must equal `tools/api-1.0.9.json`. Not compared (documented):
file names/lines/doc comments, `isInternal()`, attributes (`#[\Override]`), `isPromoted()`.

## Architecture

### One native core, one driver class

All drivers derive from the native `AbstractDriver`, whose object carries a Rust `Core`
(callbacks, enable queues, the callback queue, microtasks, the error handler, the interrupt,
the callback fiber, the suspension map). The library's four driver classes are replaced by one,
`Driver\NativeDriver`, over the native reactor (epoll/kqueue/WSAPoll); `getHandle()` is null,
`REVOLT_DRIVER` is ignored, `DriverFactory::create()` and the loop make a NativeDriver
(DIVERGENCES.md).

### Callback records

A callback is a Rust record (id number, kind, closure + its resolved function/this/scope,
stream zval + fd / signal / interval + expiration + repeat, enabled/invokable/referenced) in
a per-driver map keyed by the id number. Ids are Revolt's (`"a"`, `"b"`, ..., `"z"`, `"aa"`,
i.e. `str_increment`), a bijection with integers: lookups parse the id (no string hashing).
Ids are never reused, so "isset(callbacks[id])" is a map lookup by number. PHP
`DriverCallback` objects exist only when PHP code needs them (a userland `AbstractDriver`
subclass's activate/deactivate/enqueueCallback): created on demand with the record's values,
kept per record, written through when the record changes.

### Userland subclasses of AbstractDriver

At object creation the class's methods are inspected once: if `activate`/`dispatch`/
`deactivate`/`now` are PHP methods, the core calls them (with DriverCallback objects) instead
of the native reactor; public methods the subclass overrides (`cancel`, `enable`, ...) are
called through PHP wherever Revolt calls `$this->method()` internally (e.g. cancel of a
fired defer, disable+enable of a repeat). `EventLoop::*` take the native fast path only for
methods the active driver does not override. User `Driver` implementations (no
AbstractDriver) are called through PHP method calls.

### Run loop and fibers

- Revolt runs the loop in a "loop fiber" so `{main}` can suspend. Natively the loop is a
  re-entrant state machine (phase: start / tick / after-tick) that runs on `{main}`'s stack
  (run() and `{main}` suspensions only happen there) and returns to its caller on an
  interrupt (`{main}` resumed, uncaught error); its state lives in the Core, not on the stack.
  `isRunning()` = started and not finished (true while `{main}` holds an interrupt), as in
  Revolt where the loop fiber is then suspended.
- Callbacks and microtasks run in a **callback fiber**: a real `\Fiber` whose function is a
  Closure over a static internal function (strict types, like Revolt's files), so user
  callbacks can suspend. The fiber yields to the loop with an internal marker (a Core flag);
  any other suspension means the callback suspended: the fiber is abandoned (owned by its
  Suspension) and a new one is created on the next batch. A callback fiber that was
  destroyed (shutdown destructors, GC) is recreated (Revolt issue #105).
- Fiber switches use `zend_fiber_start/resume/suspend` (PHP 8.4+) or the `Fiber` methods'
  internal handlers via `zend_call_known_function` (8.2/8.3).
- The error handler runs in a fresh fiber, as in Revolt.
- `DriverSuspension` is native: weak map fiber => suspension (zend weakrefs hash), `{main}`'s
  suspension, the strong `suspendedFiber` while pending, dead-`{main}` handling and the
  "Event loop terminated without resuming the current suspension" error with the suspended
  fibers' traces (ReflectionFiber, cold path).
- `gc_collect_cycles()` after the loop empties, and re-run if anything was collected and the
  loop was not stopped (Revolt's runCallback).

### Reactor (src/backend)

- Level-triggered readiness (Revolt's semantics: a callback fires every tick while ready).
  Linux: epoll (EPOLLIN/EPOLLOUT, no EPOLLET); timeouts rounded up to whole ms
  (epoll_pwait2 when available is a later optimisation). macOS/BSD: kqueue without EV_CLEAR,
  ns timeouts. Windows: WSAPoll for sockets; streams that are not sockets are polled through
  PHP's own `php_select` with a zero timeout and a short poll interval (what stream_select
  does there). Registrations are per fd (several callbacks/streams on one fd share it);
  EEXIST/ENOENT are reconciled, EPERM (regular files on epoll) = always ready, like select.
- PHP stream buffers: before a blocking poll, readable streams with data in PHP's read buffer
  are reported readable at once (stream_select does this; ev/uv do not; we do it for all
  driver classes — never worse, documented).
- Timers: a min-heap keyed by (expiration, insertion sequence), so equal expirations fire in
  creation order. The clock is hrtime's (monotonic) as in Revolt's `now()`.
- Signals: native (no ext-pcntl needed) via `sigaction` + a process-wide per-signal counter
  and the loop's wake pipe (async-signal-safe); the previous disposition is restored when
  the last watcher of a signal goes (Revolt restores SIG_DFL). EINTR from any signal returns
  to PHP at once so pcntl/async handlers and timeouts run. Windows: UnsupportedFeatureException.
- fork: pthread_atfork marks the child; the next dispatch in the child rebuilds the poller
  (kqueue is not inherited; an inherited epoll instance must not be shared) and re-registers.

### GC, lifetimes, safety

- Every native object reports the zvals it holds (closures, streams, args, fibers, the error
  handler, DriverCallback objects) in `get_gc`, so driver <-> closure cycles are collected.
- No Rust borrow is held across a call into PHP (PHP may re-enter or bail out); state is
  re-fetched after every call. A bailout (fatal error) may skip Rust frames: nothing on those
  frames owns anything that needs dropping.
- Thread-local state holding zvals is released at RSHUTDOWN; ZTS: everything is per thread.
- `clone` of drivers/suspensions is refused (native state cannot be duplicated safely).

### Embedding (two copies, one loop)

- `revolt_rs::module::{minit, rinit, rshutdown, mshutdown, minfo}` are called by the host
  module (the standalone `revolt` extension or an embedder). At MINIT the first copy that
  finds no `revolt\eventloop` class registers all classes ("owner"); later copies (another
  embedder or the standalone extension, possibly another revolt-rs version) register nothing
  and stay dormant.
- The owner exports a versioned C-ABI function table (`RevoltRsAbi`, `abi_major`, size,
  version string). A copy finds the active table from the handler address of the registered
  `Revolt\EventLoop::getDriver` (dladdr + dlsym of `revolt_rs_abi` on Unix,
  GetModuleHandleEx + GetProcAddress on Windows); if it is its own handler, it calls itself.
- `revolt_rs::embed::NativeLoop::current()` gives the embedder the native fast path when the
  active driver is a native revolt-rs driver of a compatible owner (not a PHP subclass of
  AbstractDriver, not a TracingDriver or user Driver): fd watchers (raw fd/SOCKET, no PHP
  stream), timers (delay/repeat), defer/queue, cancel/enable/disable/(un)reference,
  getSuspension/suspend/resume/throw for `{main}` and fibers — Rust callbacks
  `extern "C" fn(data, id)` with an optional release hook and an optional `held` object (owned
  by the record, reported to the cycle collector); ids are real Revolt callbacks (visible to
  getIdentifiers()). `hold()`/`unhold()` keep objects alive for the running fiber where the
  collector sees them (the fiber's get_gc), so a fiber waiting natively on a driver that became
  garbage is collected. Otherwise the embedder falls back to calling `Revolt\EventLoop` through
  PHP (`embed::php`), which also covers the PHP library. Details: EMBEDDING.md.
- Composer: an embedder should declare `provide: {"ext-revolt": "*", "revolt/event-loop":
  "<the revolt/event-loop version its embedded revolt-rs implements>"}`.

## Milestones

1. Skeleton: workspace, build scripts, config.m4/Makefile.frag/composer.json, LICENSE,
   carried test suite, the reference API dump of the PHP library.
2. Zend layer + registration of the whole API (stubs) -> reflection parity clean.
3. Core loop + StreamSelectDriver on the native reactor + EventLoop facade + DriverSuspension
   + FiberLocal + errors + CallbackType -> most of the carried suite passes.
4. Signals, userland AbstractDriver subclasses, user Drivers, TracingDriver, DriverFactory,
   Ev/Event/Uv drivers, TimerQueue, ClosureHelper -> whole suite passes or is NOT_APPLICABLE.
5. Robustness: GC handlers, destruction mid-suspension, shutdown order, fork; ASan clean.
6. Embedding API + example embedder + tests (two copies both orders, foreign fast path, PHP
   fallback against the PHP library).
7. Benchmarks, README, DIVERGENCES.md, CI workflows (tests + release), final pass.
