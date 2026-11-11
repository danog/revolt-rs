# CHECKPOINT (hand-over notes)

Read PLAN.md first (architecture, API inventory, milestones), then this. User docs: README.md,
EMBEDDING.md, DIVERGENCES.md.

## Done

- M1–M5 (previous checkpoint): workspace, all 25 classes (Reflection dump identical to the PHP
  library on 8.2–8.5), native core/reactor/signals/fibers, carried suite passing, ASan clean.
- Divergences: every entry of DIVERGENCES.md is pinned by a test of `tests/native` (namespace
  RevoltRs\Tests). Divergence tests use the `NativeOnly` trait (skip under the baseline);
  everything else in tests/native also passes against the PHP library
  (`REVOLT_RS_PHPLIB=1 tools/test.sh`), which is how parity was established for userland
  drivers, user Driver via setDriver, TracingDriver dump, fork, GC of cycles, error handler
  exceptions, nested run(), exit() in a callback and shutdown with pending callbacks (phpt).
- Embedding (M6): `examples/embedder` = extension `revolt_embed_example` (functions `_mode`,
  `_owner`, `_sleep`, `_wait_readable`, `_delay`, `_released`; env
  `REVOLT_EMBED_EXAMPLE_NO_REGISTER=1` → `module::minit_with(.., false)`). `tests/embedder`
  (testsuite "embedder", run by `tools/embed-test.sh`, which builds the example, the standalone
  ext and two ABI-mismatch builds via the revolt-rs features `test-abi-major-mismatch` /
  `test-abi-size-mismatch` into target/abi-*): embedder alone, both load orders (classes'
  module checked through getExtensionName()), ABI major/size mismatch → PHP fallback,
  TracingDriver / user Driver / PHP subclass → PHP fallback, PHP library → fallback, pending
  native callbacks released when the driver is collected. ABI changes made while testing
  (unpublished, so still v1): callbacks take a `held` object (owned by the record, reported to
  the GC), and `hold`/`unhold` (inflight values of any fiber: `zend::fiber::inflight` is now a
  per-fiber stack and can `track()` user fibers) — without them a fiber waiting natively on a
  garbage driver could never be collected. Linux builds link with `-Bsymbolic`.
- EMBEDDING.md, README.md (install, Composer provide semantics, compatibility, performance,
  tests), PLAN.md updated.
- Benchmarks: `tools/bench/bench.php` (cases) + `tools/bench/run.sh [rounds]` (release LTO
  build, alternating rounds, medians, Markdown table; PHP library with StreamSelect/Ev/Uv/Event
  drivers when the extensions are in extension_dir). Numbers in README (Apple M3 Pro, 8.5 NTS).
- CI: `.github/workflows/tests.yml` (Linux/macOS/Windows × 8.2–8.6 × NTS/ZTS minus Windows 8.6:
  build, Reflection diff vs the library dump of the same PHP, suites against the extension and
  the library, embedder suite; ASan job on Linux 8.2/8.5; PIE source build), `release.yml`
  (PIE binaries, modelled on the mongo driver's), `tools/release/build-linux.sh`.
- Windows: type-checked (not built) with `cargo +nightly check --target x86_64-pc-windows-msvc`
  against a patched ext-php-rs (DOCS_RS stub bindings replaced by the macOS-generated ones with
  a vectorcall zif_handler, a fake PHP printing a VS17 build id): clean, warning-free. Linux
  type-checked the same way.

## Results (local, this Mac)

- PHP 8.5 with ev/event/uv: 525 tests = carried 470 pass (+1 NOT_APPLICABLE excluded), native
  45 (44 pass, 1 Windows-only skip), embedder 10 (skipped by tools/test.sh without the env;
  10/10 by tools/embed-test.sh, also on 8.2). Baseline against the PHP library: all pass
  (divergence tests skip). 8.2/8.3/8.4: pass.
- ASan (macOS, nightly): `tools/asan.sh test`: no reports; the 9 subprocess tests (6 phpt,
  SetDriverTest's 3 separate-process tests) cannot run under ASan on macOS (SIP strips DYLD_*
  in /bin/sh): they fail there without reports; their bodies were run by hand under ASan:
  clean. `tools/asan.sh embed`: 8 pass, 2 skip (no instrumented mismatch builds), no reports.
- Benchmarks re-measured at 3218edf (README).

## CI (first runs on GitHub)

The first run (f5552ad) failed everywhere; fixed since:
- Linux: `apt-get install libclang-dev` hit a DNS failure; the runner image already has
  libclang, which is now used when present.
- PHP 8.6: setup-php's 8.6 has API 20261112 (php-src bumped it after RC2); ext-php-rs-build
  0.2.0 only accepts 20260924 → `patches/ext-php-rs-build` + `[patch.crates-io]` (remove once
  an ext-php-rs-build release accepts it; embedders need the same patch).
- Windows: link errors for zend_ce_closure/module_registry/zend_empty_string (our own extern
  block in zend/sys.rs had no `#[link(name = "php8[ts]")]`, so no `__imp_` import).
- macOS: upstream's timing tests (`testCorrectTimeoutIfBlocking*`, `testTimerIntervalCounted*`)
  overshot their 100 ms bounds: macOS timer coalescing of nanosleep()/kevent timeouts (5–10%,
  more on CI). Timed waits now use a one-shot EVFILT_TIMER with NOTE_CRITICAL (precise to
  ~0.1 ms locally, was ~10 ms on a 200 ms timer).
- Every suite step now runs even when an earlier one failed.

## How to build and test

See README "Tests" and the previous commands:
```
cargo build --profile fast -p revolt-ext
tools/test.sh ; REVOLT_RS_PHPLIB=1 tools/test.sh ; tools/embed-test.sh
CARGO_TARGET_DIR=target/php8.2 PHP=/opt/homebrew/opt/php@8.2/bin/php PHP_CONFIG=/opt/homebrew/opt/php@8.2/bin/php-config tools/embed-test.sh
PHP=/opt/homebrew/opt/php@8.2/bin/php EXT=$PWD/target/php8.2/fast/librevolt.dylib tools/test.sh
tools/asan.sh build && tools/asan.sh test && tools/asan.sh embed
EXT=$PWD/target/release/librevolt.dylib tools/bench/run.sh 5
```
Never edit a shell script while it runs (bash reads it incrementally). Never `pkill` broad
patterns. Do not rebuild a target dir while a test run uses it.

## Next steps

1. Check the CI run of the latest push once; fix what remains (Windows has never run the
   suites; the Linux ASan job has never run: PHP's CLI loads extensions without RTLD_DEEPBIND
   since 8.2 (zend_dl_use_deepbind), which ASan would refuse).
2. Integrate the embedding into danog/mongo-php-async-driver (separate step), then its workload
   benchmark.
3. Performance passes (e.g. epoll_pwait2, avoiding the id-string allocation per callback,
   caching `callee_of`); the `defer` case is dominated by PHP closure calls.

## Open problems / notes

- `zend::zval::Z` = `ManuallyDrop<sys::zval>`: never hold a bare `sys::zval` by value.
- Values held across calls made from fibers go through `zend::fiber::inflight` (a per-fiber
  stack of frames; LIFO).
- The PHP-call fallback of an embedder keeps its `$suspension` in a native frame (invisible to
  the GC): a fiber waiting through the fallback on a garbage driver is not collected (only the
  native path has `hold`).
