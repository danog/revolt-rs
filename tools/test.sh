#!/bin/bash
# Runs a PHPUnit suite of tests/ against the extension (PHP library absent).
#   tools/test.sh [phpunit args...]          e.g. tools/test.sh --testsuite revolt
# Env: PHP (default php), EXT (default target/fast/librevolt.{so,dylib}), EXTRA_INI (more ini lines,
# e.g. "extension=ev" to run EvDriverTest).
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PHP=${PHP:-php}
if [ -z "$EXT" ]; then
  EXT=$ROOT/target/fast/librevolt.so
  [ -f "$EXT" ] || EXT=$ROOT/target/fast/librevolt.dylib
fi
cd "$ROOT/tests"
# the NOT_APPLICABLE tests are excluded unless a --filter is given
case " $* " in
  *" --filter "*) ;;
  *)
    na=$(sed -n 's/^- `\([^`]*\)`.*/\1/p' NOT_APPLICABLE.md | sed 's/\\/\\\\/g' | paste -sd'|' -)
    [ -n "$na" ] && set -- --filter "/^(?!.*($na)( |$)).*$/" "$@"
    ;;
esac
# tools/asan.sh: the ASan runtime is preloaded here (macOS drops DYLD_* when it starts a shell)
if [ -n "$REVOLT_ASAN_RT" ]; then
  if [ "$(uname)" = Darwin ]; then export DYLD_INSERT_LIBRARIES=$REVOLT_ASAN_RT; else export LD_PRELOAD=$REVOLT_ASAN_RT; fi
  export USE_ZEND_ALLOC=0 ZEND_DONT_UNLOAD_MODULES=1 ASAN_OPTIONS=$REVOLT_ASAN_OPTIONS
fi
# an ini of our own (PHPRC: the phpt subprocesses get it too; no scan dir)
INI=$(mktemp -d)
printf 'extension=%s\nzend.assertions=1\nassert.exception=1\nmemory_limit=512M\n%s\n' "$EXT" "$EXTRA_INI" > "$INI/php.ini"
export PHPRC=$INI PHP_INI_SCAN_DIR=
exec "$PHP" -c "$INI/php.ini" vendor/bin/phpunit "$@"
