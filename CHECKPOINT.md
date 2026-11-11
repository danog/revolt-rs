# CHECKPOINT (hand-over notes)

Read PLAN.md first (architecture, API inventory, milestones), then this.

## Done

- M1 skeleton: workspace (root = cdylib `revolt-ext`, lib name `revolt`; `crates/revolt-rs` =
  the implementation; `examples/embedder` = empty placeholder crate), build scripts, config.m4 /
  Makefile.frag / composer.json (type php-ext, provide revolt/event-loop 1.0.9), MIT LICENSE,
  revolt/event-loop v1.0.9's tests carried into `tests/revolt` (MIT notice + README there).
- M2 API: all 25 classes registered natively. `tools/dump-api.php diff` = **identical** to the
  PHP library on PHP 8.2, 8.3, 8.4, 8.5 (the `: self` return type is version-dependent: cfg php85).
- M3/M4 behaviour: native core (`crates/revolt-rs/src/core`), reactor (epoll/kqueue/WSAPoll,
  batched interest changes, fd-reuse detection), timers (indexed heap), signals (sigaction +
  wake pipe), callback fiber, error fiber, DriverSuspension, FiberLocal (real WeakMaps),
  TracingDriver, DriverFactory, Ev/Event/Uv/StreamSelect drivers on the same reactor, TimerQueue
  (Revolt's algorithm on its own arrays), userland AbstractDriver subclass hooks (untested yet).
- Carried suite: **all pass** on PHP 8.5 with ev/event/uv loaded (470 tests, 1413 assertions,
  0 skipped) and on 8.2/8.3/8.4/8.5 without them (Ev/Event/Uv tests skip). 1 test NOT_APPLICABLE
  (FD_SETSIZE; pinned by tests/native/NoFdSetSizeLimitTest.php).
- ASan (macOS, nightly): full suite, no reports (phpt files cannot run under ASan on macOS: SIP
  strips DYLD_* in PHPUnit's /bin/sh; they would on Linux).
- `crates/revolt-rs/src/embed.rs`: the embedding API + C ABI (`revolt_rs_abi_v1` exported, found
  from the owner's `EventLoop::getDriver` handler via dladdr/dlsym) — written and compiling, NOT
  tested yet; `module::minit` registers classes only if `revolt\eventloop` is absent.
- DIVERGENCES.md drafted (entries marked *pin pending* need a test in tests/native).

## Commits

074f0c2 .. b4c4b29 (+ the checkpoint commit). Commit identity: Daniil Gentili <daniil@daniil.it>.

## How to build and test

```
cargo build --profile fast -p revolt-ext            # target/fast/librevolt.dylib (Homebrew PHP 8.5)
(cd tests && composer install)                      # phpunit 9.6, no revolt/event-loop
(cd tools/phplib && composer install)               # the PHP library: dump reference/baseline/bench
tools/test.sh                                       # both suites, NOT_APPLICABLE excluded
E=$(php-config --extension-dir); EXTRA_INI=$'extension='$E$'/ev.so\nextension='$E$'/event.so\nextension='$E/uv.so tools/test.sh
php -n -d extension=$PWD/target/fast/librevolt.dylib tools/dump-api.php > /tmp/ext.json && php tools/dump-api.php diff tools/api-1.0.9.json /tmp/ext.json
# other PHP versions: separate target dirs
CARGO_TARGET_DIR=target/php8.2 PHP=/opt/homebrew/opt/php@8.2/bin/php PHP_CONFIG=/opt/homebrew/opt/php@8.2/bin/php-config cargo build --profile fast -p revolt-ext
PHP=/opt/homebrew/opt/php@8.2/bin/php EXT=$PWD/target/php8.2/fast/librevolt.dylib tools/test.sh
(the reference dump per version: /opt/homebrew/opt/php@8.2/bin/php -n tools/dump-api.php phplib)
tools/asan.sh build && tools/asan.sh test           # ASan
```
Never `pkill` broad patterns (other agents run php/phpunit on this machine): kill own PIDs.
Do not rebuild target/fast while a test run uses it (macOS kills processes whose mapped dylib
changes).

## Next steps

1. Pin the *pin pending* divergences with tests in `tests/native` (PHPUnit, namespace
   RevoltRs\Tests): signals without pcntl + disposition restore, buffered stream data, invalid
   stream TypeError, clone refusal, DriverSuspension ctor, dead-fiber resume ignored, userland
   AbstractDriver subclass (activate/dispatch/deactivate/now hooks, override of cancel/enable/
   disable, enqueueCallback), user Driver implementation via setDriver, TracingDriver dump,
   fork (pcntl_fork: child rebuilds poller), GC of driver<->closure cycles, shutdown order,
   exit() inside a callback, exception from the error handler, nested run() errors.
2. Embedding: write `examples/embedder` (extension `revolt_embed_example`: functions
   `revolt_embed_example_mode()` -> "native"/"native-foreign"/"php", `_sleep(float)` and
   `_wait_readable($stream)` using `revolt_rs::embed::NativeLoop` or the PHP fallback; env
   `REVOLT_EMBED_EXAMPLE_NO_REGISTER=1` to skip class registration for the PHP-library fallback
   test — needs a `module::minit` variant that does not register). Tests: both load orders with
   revolt.so (one set of classes, foreign fast path works), embedder alone, embedder without
   registration + PHP library (fallback). Write EMBEDDING.md (API, ABI rules, required
   EXT_PHP_RS_ALLOWED_BINDINGS from crates/revolt-rs/BINDINGS, ext-php-rs =0.16.4 + feature
   "observer", composer provide ext-revolt + revolt/event-loop 1.0.9).
3. Benchmarks (tools/bench): defer/queue throughput, timers, onReadable ping-pong over socket
   pairs, suspension round trips; native vs PHP StreamSelectDriver vs PHP EvDriver (ev.so is in
   `php-config --extension-dir`); mongo-php-async-driver's tools/bench/fibers.php (read-only use
   of /Users/daniil/repos/mongo-php-async-driver; needs a MongoDB server). Numbers into README.
4. README (what, install with PIE, compatibility, performance, divergences link, note that
   `provide` only applies to projects requiring danog/revolt-rs, MINIT registration makes an
   installed revolt/event-loop dormant), CI workflows modelled on the mongo driver's
   .github/workflows/tests.yml and release.yml (Linux/macOS/Windows, PHP 8.2-8.6 NTS/ZTS: build,
   reflection diff against the PHP library dump of the same PHP, suite with NOT_APPLICABLE
   excluded, baseline REVOLT_RS_PHPLIB=1 run, ASan on Linux).
5. Windows: the backend (WSAPoll + PeekNamedPipe for pipes) and symbol lookup are written but
   never compiled; expect fixes from CI (nightly + vectorcall, as the mongo driver).
6. Performance passes once benchmarks exist (e.g. epoll_pwait2, avoiding the id-string
   allocation per callback, caching `callee_of`).

## Preliminary numbers (fast profile, not LTO; PHP 8.5 NTS, Apple Silicon, no opcache)

Quick script (not yet in tools/bench): 1M EventLoop::defer + run, 200k {main} and fiber
suspend/resume round trips through EventLoop::queue($s->resume(...)):

| | native | PHP StreamSelectDriver |
|---|---|---|
| defer | 1.12 M/s | 0.40 M/s |
| {main} suspend/resume | 4.02 M/s | 1.29 M/s |
| fiber suspend/resume | 3.12 M/s | 1.51 M/s |

## Open problems / notes

- `zend::zval::Z` = `ManuallyDrop<sys::zval>`: ext-php-rs gives `sys::zval` a Drop that releases;
  never hold a bare `sys::zval` by value.
- Values held across calls made from fibers go through `zend::fiber::inflight` (reported by the
  fiber's get_gc), otherwise suspended fibers in cycles are never collected.
- Microtask arguments are handed to the call (refcount transfer) so the callee holds the only
  reference (Revolt's `...($args = [])`), except for callees taking references.
- Open question for the maintainer: none blocking yet.
