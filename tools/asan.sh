#!/bin/bash
# AddressSanitizer runs (the memory-safety gate).
#   tools/asan.sh build              nightly build of the extension with -Zsanitizer=address
#   tools/asan.sh test [phpunit...]  the PHPUnit suites with that build, under ASan
#   tools/asan.sh embed [phpunit...] the embedding suite (examples/embedder) under ASan
#   tools/asan.sh php [args...]      one php process with that build, under ASan
# Env: PHP / PHP_CONFIG (the PHP to build for and run; default php / php-config), OUT (reports),
# TOOLCHAIN (the nightly Rust toolchain; default nightly).
#
# How: the extension is instrumented, PHP is not; ASan's runtime must be in the process from the
# start, so it is preloaded (DYLD_INSERT_LIBRARIES on macOS, LD_PRELOAD on Linux), and
# USE_ZEND_ALLOC=0 sends emalloc/efree to the (intercepted) system allocator, so ASan sees every
# Zend allocation too. ZEND_DONT_UNLOAD_MODULES keeps symbols for the reports. Linux: PHP < 8.5
# loads extensions with RTLD_DEEPBIND, which ASan refuses: tools/asan-nodeepbind, preloaded
# ahead of the runtime, drops the flag.
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PHP=$(command -v "${PHP:-php}")
PHP_CONFIG=$(command -v "${PHP_CONFIG:-php-config}")
TOOLCHAIN=${TOOLCHAIN:-nightly}
HOST=$(rustc +"$TOOLCHAIN" -vV | sed -n 's/^host: //p')
TD=$ROOT/target/asan
SO_DIR=$TD/$HOST/fast
case $(uname) in
  Darwin) SO=$SO_DIR/librevolt.dylib; PRE=DYLD_INSERT_LIBRARIES; NODEEPBIND=
          RT=$(rustc +"$TOOLCHAIN" --print sysroot)/lib/rustlib/$HOST/lib/librustc-nightly_rt.asan.dylib ;;
  *) SO=$SO_DIR/librevolt.so; PRE=LD_PRELOAD
     # Rust ships the Linux runtime only as a static library (for executables): preload the
     # system's shared one, clang's or else gcc's (the same compiler-rt runtime and ABI)
     RT=
     for rt in "$(clang -print-file-name="libclang_rt.asan-$(uname -m).so" 2>/dev/null)" "$(gcc -print-file-name=libasan.so 2>/dev/null)"; do
       case $rt in /*) [ -f "$rt" ] && { RT=$(readlink -f "$rt"); break; } ;; esac
     done
     NODEEPBIND=$TD/nodeepbind/release/libasan_nodeepbind.so ;;
esac
if [ "$1" != build ] && [ ! -f "$RT" ]; then
  echo "no shared ASan runtime found (Linux: install clang's compiler-rt or gcc's libasan)" >&2; exit 1
fi
OUT=${OUT:-$ROOT/target/asan-results}
mkdir -p "$OUT"
ASAN_OPTS=detect_leaks=0:abort_on_error=0:halt_on_error=1:log_path=$OUT/asan
if [ -n "$NODEEPBIND" ]; then
  # the runtime is not first in the preload list: its check of that is turned off
  RT=$NODEEPBIND:$RT ASAN_OPTS=$ASAN_OPTS:verify_asan_link_order=0
fi
env_run() {
  env $PRE="$RT" USE_ZEND_ALLOC=0 ZEND_DONT_UNLOAD_MODULES=1 ASAN_OPTIONS=$ASAN_OPTS "$@"
}
case $1 in
  build)
    cd "$ROOT" && PHP="$PHP" PHP_CONFIG="$PHP_CONFIG" CARGO_TARGET_DIR="$TD" RUSTFLAGS="-Zsanitizer=address" \
      cargo +"$TOOLCHAIN" build --profile fast --target "$HOST" -p revolt-ext -p revolt-embed-example
    if [ -n "$NODEEPBIND" ]; then
      CARGO_TARGET_DIR="$TD/nodeepbind" cargo +"$TOOLCHAIN" build --release --manifest-path tools/asan-nodeepbind/Cargo.toml
    fi
    echo "$SO" ;;
  embed)
    # the embedding suite (both load orders, fallbacks) with the instrumented builds; the ABI
    # mismatch cases skip (no instrumented mismatch builds)
    shift; rm -f "$OUT"/asan.*
    rc=0
    REVOLT_EMBED_EXT=$(dirname "$SO")/$(basename "$SO" | sed 's/revolt/revolt_embed_example/') REVOLT_EXT=$SO \
    PHP="$PHP" EXT=$SO REVOLT_ASAN_RT=$RT REVOLT_ASAN_OPTIONS=$ASAN_OPTS \
      "$ROOT/tools/test.sh" --testsuite embedder "$@" || rc=$?
    if ls "$OUT"/asan.* >/dev/null 2>&1; then echo "ASan reports in $OUT:"; grep -h "ERROR: AddressSanitizer" "$OUT"/asan.* | sort | uniq -c; rc=1; else echo "ASan: no reports"; fi
    exit $rc ;;
  test)
    shift; rm -f "$OUT"/asan.*
    rc=0
    PHP="$PHP" EXT=$SO REVOLT_ASAN_RT=$RT REVOLT_ASAN_OPTIONS=$ASAN_OPTS \
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
