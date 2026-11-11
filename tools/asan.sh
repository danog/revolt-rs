#!/bin/bash
# AddressSanitizer runs (the memory-safety gate).
#   tools/asan.sh build              nightly build of the extension with -Zsanitizer=address
#   tools/asan.sh test [phpunit...]  the PHPUnit suites with that build, under ASan
#   tools/asan.sh embed [phpunit...] the embedding suite (examples/embedder) under ASan
#   tools/asan.sh php [args...]      one php process with that build, under ASan
# Env: PHP / PHP_CONFIG (the PHP to build for and run; default php / php-config), OUT (reports).
#
# How: the extension is instrumented, PHP is not; ASan's runtime must be in the process from the
# start, so it is preloaded (DYLD_INSERT_LIBRARIES on macOS, LD_PRELOAD on Linux), and
# USE_ZEND_ALLOC=0 sends emalloc/efree to the (intercepted) system allocator, so ASan sees every
# Zend allocation too. ZEND_DONT_UNLOAD_MODULES keeps symbols for the reports.
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PHP=$(command -v "${PHP:-php}")
PHP_CONFIG=$(command -v "${PHP_CONFIG:-php-config}")
HOST=$(rustc +nightly -vV | sed -n 's/^host: //p')
TD=$ROOT/target/asan
SO_DIR=$TD/$HOST/fast
case $(uname) in
  Darwin) SO=$SO_DIR/librevolt.dylib; PRE=DYLD_INSERT_LIBRARIES
          RT=$(rustc +nightly --print sysroot)/lib/rustlib/$HOST/lib/librustc-nightly_rt.asan.dylib ;;
  *) SO=$SO_DIR/librevolt.so; PRE=LD_PRELOAD
     RT=$(rustc +nightly --print sysroot)/lib/rustlib/$HOST/lib/librustc-nightly_rt.asan.so ;;
esac
OUT=${OUT:-$ROOT/target/asan-results}
mkdir -p "$OUT"
env_run() {
  env $PRE="$RT" USE_ZEND_ALLOC=0 ZEND_DONT_UNLOAD_MODULES=1 \
    ASAN_OPTIONS=detect_leaks=0:abort_on_error=0:halt_on_error=1:log_path=$OUT/asan "$@"
}
case $1 in
  build)
    cd "$ROOT" && PHP="$PHP" PHP_CONFIG="$PHP_CONFIG" CARGO_TARGET_DIR="$TD" RUSTFLAGS="-Zsanitizer=address" \
      cargo +nightly build --profile fast --target "$HOST" -p revolt-ext -p revolt-embed-example
    echo "$SO" ;;
  embed)
    # the embedding suite (both load orders, fallbacks) with the instrumented builds; the ABI
    # mismatch cases skip (no instrumented mismatch builds)
    shift; rm -f "$OUT"/asan.*
    rc=0
    REVOLT_EMBED_EXT=$(dirname "$SO")/$(basename "$SO" | sed 's/revolt/revolt_embed_example/') REVOLT_EXT=$SO \
    PHP="$PHP" EXT=$SO REVOLT_ASAN_RT=$RT REVOLT_ASAN_OPTIONS=detect_leaks=0:abort_on_error=0:halt_on_error=1:log_path=$OUT/asan \
      "$ROOT/tools/test.sh" --testsuite embedder "$@" || rc=$?
    if ls "$OUT"/asan.* >/dev/null 2>&1; then echo "ASan reports in $OUT:"; grep -h "ERROR: AddressSanitizer" "$OUT"/asan.* | sort | uniq -c; rc=1; else echo "ASan: no reports"; fi
    exit $rc ;;
  test)
    shift; rm -f "$OUT"/asan.*
    rc=0
    PHP="$PHP" EXT=$SO REVOLT_ASAN_RT=$RT REVOLT_ASAN_OPTIONS=detect_leaks=0:abort_on_error=0:halt_on_error=1:log_path=$OUT/asan \
      "$ROOT/tools/test.sh" "$@" || rc=$?
    # macOS: phpt subprocesses start through /bin/sh, which drops DYLD_*: their "interceptors not
    # working" aborts are not reports (they run under ASan on Linux)
    grep -l "Interceptors are not working" "$OUT"/asan.* 2>/dev/null | xargs rm -f
    if ls "$OUT"/asan.* >/dev/null 2>&1; then echo "ASan reports in $OUT:"; grep -h "ERROR: AddressSanitizer" "$OUT"/asan.* | sort | uniq -c; rc=1; else echo "ASan: no reports"; fi
    exit $rc ;;
  php)
    shift; env_run "$PHP" -n -d extension="$SO" "$@" ;;
  *) echo "usage: $0 build|test|embed|php"; exit 2 ;;
esac
