#!/bin/bash
# Runs a PHPUnit suite of tests/ against the extension (PHP library absent).
#   tools/test.sh [phpunit args...]          e.g. tools/test.sh --testsuite revolt
# Env: PHP (default php), EXT (default target/fast/librevolt.{so,dylib}), EXTRA_INI (more ini lines,
# e.g. "extension=pcntl"), REVOLT_RS_PHPLIB=1 (the baseline: the PHP library
# from tools/phplib instead of the extension), REVOLT_TEST_SCAN_DIR=1 (keep the extensions of PHP's
# own configuration: its ini scan dir and the extension lines of its php.ini),
# REVOLT_TEST_CI_EXCLUDE=<name> (CI only: also exclude the tests of tests/ci-excluded/<name>.md,
# the ones a CI runner's timers break, for the PHP library too).
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PHP=${PHP:-php}
if [ -z "$EXT" ]; then
  EXT=$ROOT/target/fast/librevolt.so
  [ -f "$EXT" ] || EXT=$ROOT/target/fast/librevolt.dylib
fi
cd "$ROOT/tests"
# the NOT_APPLICABLE tests (and the CI's excluded ones) are excluded unless a --filter is given;
# a list entry is the end of a test name: `Class::method`, `::method` (every class) or `Class::*`
# (every test of the class)
case " $* " in
  *" --filter "*) ;;
  *)
    lists=NOT_APPLICABLE.md
    if [ -n "$REVOLT_TEST_CI_EXCLUDE" ]; then
      [ -f "ci-excluded/$REVOLT_TEST_CI_EXCLUDE.md" ] || { echo "no tests/ci-excluded/$REVOLT_TEST_CI_EXCLUDE.md" >&2; exit 1; }
      lists="$lists ci-excluded/$REVOLT_TEST_CI_EXCLUDE.md"
    fi
    na=$(sed -n 's/^- `\([^`]*\)`.*/\1/p' $lists | sed 's/\\/\\\\/g; s/\*/[^ ]*/g' | paste -sd'|' -)
    [ -n "$na" ] && set -- --filter "/^(?!.*($na)( |$)).*$/" "$@"
    ;;
esac
# tools/asan.sh: the ASan runtime is preloaded here (macOS drops DYLD_* when it starts a shell)
if [ -n "$REVOLT_ASAN_RT" ]; then
  if [ "$(uname)" = Darwin ]; then export DYLD_INSERT_LIBRARIES=$REVOLT_ASAN_RT; else export LD_PRELOAD=$REVOLT_ASAN_RT; fi
  export USE_ZEND_ALLOC=0 ZEND_DONT_UNLOAD_MODULES=1 ASAN_OPTIONS=$REVOLT_ASAN_OPTIONS
fi
# an ini of our own (PHPRC: the phpt subprocesses get it too); PHP's own extensions are dropped
# unless REVOLT_TEST_SCAN_DIR=1 (CI: the extensions setup-php installs, PHPUnit's dom/mbstring...)
INI=$(mktemp -d)
case $(uname -s) in MINGW*|MSYS*|CYGWIN*) INI=$(cygpath -m "$INI") ;; esac
EXTLINE="extension=$EXT"
# baseline: the phpt tests' `require 'vendor/autoload.php'` finds tools/phplib's (the library)
# through the include path (PHPUnit resets auto_prepend_file for them)
if [ -n "$REVOLT_RS_PHPLIB" ]; then
  case $(uname -s) in MINGW*|MSYS*|CYGWIN*) EXTLINE="include_path=\"$(cygpath -m "$ROOT/tools/phplib");.\"" ;; *) EXTLINE="include_path=\"$ROOT/tools/phplib:.\"" ;; esac
fi
printf '%s\nzend.assertions=1\nassert.exception=1\nmemory_limit=512M\n%s\n' "$EXTLINE" "$EXTRA_INI" > "$INI/php.ini"
if [ -n "$REVOLT_TEST_SCAN_DIR" ]; then
  # the extensions of PHP's own php.ini too (ours replaces it; Windows has no scan dir)
  "$PHP" -r '$f = php_ini_loaded_file(); foreach ($f ? file($f) : [] as $l) { if (preg_match("/^\s*(zend_)?extension(_dir)?\s*=/", $l)) echo $l; }' >> "$INI/php.ini"
else
  export PHP_INI_SCAN_DIR=
fi
export PHPRC=$INI
exec "$PHP" -c "$INI/php.ini" vendor/bin/phpunit "$@"
