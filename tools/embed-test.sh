#!/bin/bash
# Builds examples/embedder, the standalone extension and its two ABI-mismatch test builds, then
# runs the embedder suite (tests/embedder) against them.
#   tools/embed-test.sh [phpunit args...]
# Env: PHP / PHP_CONFIG (the PHP to build for and run), PROFILE (default fast), CARGO_TARGET_DIR.
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE=${PROFILE:-fast}
TD=${CARGO_TARGET_DIR:-$ROOT/target}
case $(uname -s) in
  Darwin) pre=lib; ext=dylib ;;
  MINGW*|MSYS*|CYGWIN*) pre=; ext=dll ;;
  *) pre=lib; ext=so ;;
esac
cd "$ROOT"
CARGO_TARGET_DIR=$TD cargo build --profile "$PROFILE" -p revolt-ext -p revolt-embed-example
for f in major size; do
  CARGO_TARGET_DIR=$TD/abi-$f cargo build --profile "$PROFILE" -p revolt-ext --features revolt-rs/test-abi-$f-mismatch
done
export REVOLT_EMBED_EXT=$TD/$PROFILE/${pre}revolt_embed_example.$ext
export REVOLT_EXT=$TD/$PROFILE/${pre}revolt.$ext
export REVOLT_EXT_ABI_MAJOR=$TD/abi-major/$PROFILE/${pre}revolt.$ext
export REVOLT_EXT_ABI_SIZE=$TD/abi-size/$PROFILE/${pre}revolt.$ext
EXT=$REVOLT_EXT exec "$ROOT/tools/test.sh" --testsuite embedder "$@"
