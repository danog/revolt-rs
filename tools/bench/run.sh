#!/bin/bash
# Runs tools/bench/bench.php against the extension (release build: fat LTO) and against the PHP
# library with each driver whose extension is available (StreamSelectDriver always; EvDriver,
# UvDriver, EventDriver when ev/uv/event are in PHP's extension dir), alternating rounds, and
# prints the median of each case as a Markdown table (operations per second).
#   tools/bench/run.sh [rounds]        Env: PHP, PHP_CONFIG, EXT (skip the build), OPCACHE (default 1),
#                                      EDIR (where ev.so/uv.so/event.so are)
set -e
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
PHP=${PHP:-php}
ROUNDS=${1:-5}
if [ -z "$EXT" ]; then
  (cd "$ROOT" && cargo build --release -p revolt-ext >&2)
  EXT=$ROOT/target/release/librevolt.so
  [ -f "$EXT" ] || EXT=$ROOT/target/release/librevolt.dylib
fi
# where ev/uv/event are: the configured extension_dir (PECL installs), else the built-in one
EDIR=${EDIR:-$("$PHP" -r 'echo ini_get("extension_dir");')}
BASE=(-n -d opcache.enable_cli=${OPCACHE:-1} -d memory_limit=1G)
names=(native lib-stream-select)
cmds=("${BASE[*]} -d extension=$EXT $ROOT/tools/bench/bench.php default"
      "${BASE[*]} $ROOT/tools/bench/bench.php default")
for d in ev uv event; do
  if [ -f "$EDIR/$d.so" ]; then
    names+=("lib-$d"); cmds+=("${BASE[*]} -d extension=$EDIR/$d.so $ROOT/tools/bench/bench.php $d")
  fi
done
OUT=$(mktemp -d)
for r in $(seq "$ROUNDS"); do
  for i in "${!names[@]}"; do
    # shellcheck disable=SC2086
    "$PHP" ${cmds[$i]} >> "$OUT/${names[$i]}"
    echo "round $r ${names[$i]} done" >&2
  done
done
"$PHP" -n -r '
$dir = $argv[1]; $names = array_slice($argv, 2); $med = [];
foreach ($names as $n) {
    foreach (file("$dir/$n", FILE_IGNORE_NEW_LINES) as $l) { [$c, $v] = explode(" ", $l); $med[$c][$n][] = (float) $v; }
}
echo "| case | ", implode(" | ", $names), " |\n|---|", str_repeat("---|", count($names)), "\n";
foreach ($med as $c => $byName) {
    $row = [];
    foreach ($names as $n) { $v = $byName[$n]; sort($v); $m = $v[intdiv(count($v), 2)]; $row[$n] = $m; }
    $cells = [];
    foreach ($names as $n) {
        $cells[] = sprintf("%.2f M/s", $row[$n] / 1e6) . ($n === "native" ? "" : sprintf(" (native %.1f×)", $row["native"] / $row[$n]));
    }
    echo "| $c | ", implode(" | ", $cells), " |\n";
}' "$OUT" "${names[@]}"
