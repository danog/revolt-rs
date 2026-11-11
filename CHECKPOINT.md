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

Run 43042104542 (dae20ed): 19 failures. Fixes pushed since:
- f258565: build on PHP 8.6.0 (php_stream_cast lost its underscore; handlers' offset size_t).
- 33f09ce: embedder SleepingDriver enqueues timers expired together in expiration order.
- 4c90dad: asan.sh preloads the system's shared ASan runtime on Linux; TOOLCHAIN pins nightly.
- fa12f32: test.sh keeps php.ini's extensions on Windows (no scan dir there).
- b134077: CI: VS16 linker 14.29 for Windows PHP 8.2/8.3, mbstring, macOS `taskpolicy -a`,
  no ev/event on 8.6.
- 0ba25a0: ReflectionTest uses an INET socket pair on Windows.
- 7d08489: tools/asan-nodeepbind: dlopen shim dropping RTLD_DEEPBIND (ASan on PHP < 8.5).
- 5ddfa32: Windows bindgen uses CLANG_PATH=C:\Program Files\LLVM\bin\clang.exe; ASan job
  installs tools/phplib.

Run 43052710122 (b134077): all PIE builds, ubuntu 8.2/8.2 ZTS/8.3 ZTS/8.6/8.6 ZTS, macOS 8.6
ZTS passed. macOS timer step: `taskpolicy -a` gives no improvement (sleeps overshoot
+14..+73 ms either way) → to be removed.

Run 43054345552 (5ddfa32), partial: Windows 8.2, 8.2 ZTS, 8.3 (14.29-linked DLL accepted), 8.5,
ASan 8.5, PIE 8.4/8.4 ZTS passed. Windows 8.3 ZTS and 8.4 ZTS failed only
`StreamSelectDriverTest::testCorrectTimeoutIfBlockingBeforeDelay` (1.4981 s resp. 1.4994 s,
bound ≥ 1.5; the 8.4 ZTS one in the PHP-library baseline): Windows' usleep() (a waitable timer)
returns a few ms early → flaky upstream test on Windows runners.

Also in 43054345552: ASan 8.2 passed with the deepbind shim (525 tests, "ASan: no reports",
embed 10/2 skipped), so ASan stays on 8.2 + 8.5.

f5b6ed6 + 5d6217b: `REVOLT_TEST_CI_EXCLUDE=macos|windows` (set by the workflow per OS) makes
tools/test.sh also exclude `tests/ci-excluded/<name>.md` (same list format as NOT_APPLICABLE.md;
an entry `::method` matches every driver class); macOS: testCorrectTimeoutIfBlockingBefore
{Delay,Activate}, Windows: ...BeforeDelay. `taskpolicy -a` removed; the informational timer step
now runs on macOS and Windows and prints usleep()'s deviation per hrtime() and microtime().
Run 43057031372 (5d6217b): pending.

Maintainer decision (macOS; applied to Windows the same way): upstream timing tests failing on
CI runners because of the OS's timer behaviour (the PHP library fails them too) are excluded on
that CI only, through an env-gated list in tools/test.sh with the measured deviation as the
reason, NOT in tests/NOT_APPLICABLE.md (which excludes everywhere).

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

1. CI green (in progress, see "CI" above): open items are the per-CI timing exclusions, the
   remaining jobs of the current run (ASan 8.2 with the deepbind shim; fallback: ASan on 8.5/8.6
   only, documented), ubuntu and macOS jobs.
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
