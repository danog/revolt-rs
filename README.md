# revolt-rs

[revolt/event-loop](https://github.com/revoltphp/event-loop) 1.0.9, implemented natively in Rust as
a PHP extension (`revolt`). It registers the same classes at startup, so Composer's autoloader
never loads the PHP versions, and everything built on Revolt (Amp v3, your own code) runs
unchanged on a native loop.

- **Same API and behaviour:** every class, interface, enum, method, parameter, default and
  constant of revolt/event-loop 1.0.9, with an identical Reflection dump (checked on every PHP
  version in CI). revolt/event-loop's own test suite passes against the extension unchanged
  (one test does not apply: there is no FD_SETSIZE limit to hit). Every difference is listed,
  with the test that pins it, in [DIVERGENCES.md](DIVERGENCES.md).
- **Faster:** 2–8× the PHP library's throughput for callbacks, timers and suspensions, 1.4–1.6×
  for socket I/O; see [Performance](#performance).
- **Native reactor:** epoll (Linux), kqueue (macOS, BSD), WSAPoll (Windows), for every driver
  class: no `stream_select()` limits, and no need for ext-ev, ext-event or ext-uv. Signals work
  without ext-pcntl.
- **Embeddable:** other Rust extensions can embed it (one loop per thread, whoever registered the
  classes) and drive the loop from Rust without PHP calls; see [EMBEDDING.md](EMBEDDING.md).
- **Pure Rust:** no C libraries; the only C is the small `wrapper.c` of
  [ext-php-rs](https://github.com/extphprs/ext-php-rs), the Zend Engine bindings. PHP 8.2 to
  8.6, NTS and ZTS (each PHP thread has its own loop).

## Install

With [PIE](https://github.com/php/pie):

```bash
pie install danog/revolt-rs
```

Releases carry pre-built binaries for Linux (x86-64 and arm64, glibc and musl), macOS (arm64 and
x86-64) and Windows (x86-64), for PHP 8.2 to 8.6, NTS and ZTS (PHP 8.6, until its release: no
NTS build for Linux, none for Windows). Elsewhere, PIE builds from source, which needs Rust (`cargo`)
and libclang.

Nothing else changes: keep requiring revolt/event-loop (or amphp/amp) in `composer.json`. With
the extension loaded, the installed package stays unused: its classes already exist when the
autoloader would load them.

### Composer: `provide`

The package danog/revolt-rs declares `"provide": {"revolt/event-loop": "1.0.9"}`. That only
affects projects that require danog/revolt-rs through Composer: Composer then counts it as
revolt/event-loop 1.0.9 and does not install the PHP library. Projects that do not require it
(the usual case: the extension is installed with PIE, the project requires amphp/amp) get
revolt/event-loop installed as before, and it stays dormant, because the extension registers the
classes at MINIT, before any autoloader runs.

Extensions that embed revolt-rs should provide `ext-revolt` and the same revolt/event-loop
version as the revolt-rs they embed (see [EMBEDDING.md](EMBEDDING.md#composer)).

## Compatibility

| | |
|---|---|
| revolt/event-loop | 1.0.9: the API (Reflection-identical) and behaviour of every class |
| PHP | 8.2, 8.3, 8.4, 8.5, 8.6; NTS and ZTS; non-debug builds; 64-bit |
| OS | Linux and macOS (x86-64, arm64), Windows (x86-64); the BSDs' kqueue backend is the macOS one, untested there |
| Drivers | `StreamSelectDriver`, `EvDriver`, `EventDriver`, `UvDriver` all run on the native reactor (the last three keep their `isSupported()` and `getHandle()`), `TracingDriver`, user `Driver` implementations and PHP subclasses of `AbstractDriver` work as in Revolt |
| Differences | [DIVERGENCES.md](DIVERGENCES.md), each pinned by a test of `tests/native` |

## Performance

Operations per second, the extension against the PHP library with each of its drivers, measured
on 2026-11-11 at 98b2cab: Apple M3 Pro (macOS), PHP 8.5.12 NTS CLI with opcache (no JIT),
release build (fat LTO), ext-ev 1.2.3, ext-uv 0.3.0, ext-event 3.1.5; 5 alternating rounds,
medians. `tools/bench/run.sh` reproduces it (`tools/bench/bench.php` has the cases).

| Case | revolt-rs | PHP: StreamSelectDriver | PHP: EvDriver | PHP: UvDriver | PHP: EventDriver |
|---|---|---|---|---|---|
| `defer()`: 1M callbacks | 1.89 M/s | 0.94 M/s (2.0×) | 0.91 M/s (2.1×) | 0.92 M/s (2.1×) | 0.90 M/s (2.1×) |
| `queue()`: 1M microtasks | 6.79 M/s | 3.01 M/s (2.3×) | 2.83 M/s (2.4×) | 3.01 M/s (2.3×) | 3.00 M/s (2.3×) |
| `delay()`: 200k timers, 0–10 ms | 0.90 M/s | 0.20 M/s (4.5×) | 0.66 M/s (1.4×) | 0.20 M/s (4.5×) | 0.59 M/s (1.5×) |
| `repeat()`: 1000 timers × 200 | 4.37 M/s | 0.52 M/s (8.4×) | 1.59 M/s (2.7×) | 0.95 M/s (4.6×) | 1.29 M/s (3.4×) |
| `onReadable()`: socket-pair ping-pong (round trips) | 0.40 M/s | 0.25 M/s (1.6×) | 0.28 M/s (1.4×) | 0.28 M/s (1.4×) | 0.29 M/s (1.4×) |
| `{main}` suspend/resume round trips | 3.48 M/s | 1.08 M/s (3.2×) | 1.08 M/s (3.2×) | 1.08 M/s (3.2×) | 1.07 M/s (3.3×) |
| fiber suspend/resume round trips | 2.62 M/s | 1.21 M/s (2.2×) | 1.21 M/s (2.2×) | 1.19 M/s (2.2×) | 1.20 M/s (2.2×) |

In parentheses: how many times faster revolt-rs is. Every case calls PHP closures, whose cost
the extension cannot remove; the loop's own overhead is what shrinks.

## Build from source

```bash
PHP=/usr/bin/php8.4 PHP_CONFIG=/usr/bin/php-config8.4 cargo build --release -p revolt-ext
# load target/release/librevolt.so (.dylib on macOS) with extension=...
```

Or the phpize way (what PIE does): `phpize && ./configure && make && make install`.

On Windows, set `PHP` to php.exe (there is no php-config; ext-php-rs downloads the matching PHP
development pack) and use nightly Rust: ext-php-rs needs the vectorcall calling convention, which
only nightly has.

```bash
PHP='C:\tools\php\php.exe' cargo +nightly build --release -p revolt-ext
# target\release\revolt.dll
```

## Tests

```bash
(cd tests && composer install) && (cd tools/phplib && composer install)
cargo build --profile fast -p revolt-ext
tools/test.sh                                # tests/revolt (revolt/event-loop's suite) + tests/native
REVOLT_RS_PHPLIB=1 tools/test.sh             # the same against the PHP library (the baseline)
tools/embed-test.sh                          # the embedding tests (builds examples/embedder)
php -n tools/dump-api.php phplib > ref.json && php -n -d extension=target/fast/librevolt.so tools/dump-api.php > ext.json && php tools/dump-api.php diff ref.json ext.json
tools/asan.sh build && tools/asan.sh test    # AddressSanitizer (nightly Rust)
```

`tests/revolt` is revolt/event-loop's suite, unchanged; [tests/NOT_APPLICABLE.md](tests/NOT_APPLICABLE.md)
lists the test that cannot apply and why. `tests/native` holds ours: one test per divergence
(skipped against the PHP library) and robustness tests (userland drivers, GC of cycles through
the loop, fork, shutdown, exit() in callbacks) that pass on both.

## License

MIT (see [LICENSE](LICENSE)). `tests/revolt` is revolt/event-loop's test suite, MIT, by its
authors (see its notice).
