# revolt-rs

[revolt/event-loop](https://github.com/revoltphp/event-loop), implemented natively in Rust as
a PHP extension (`revolt`). It registers the same classes at startup, so the PHP versions never
load, and everything built on Revolt (Amp v3, your own code) runs unchanged on a native loop, 2–8×
faster.

- **Same API:** Reflection-identical to revolt/event-loop; its own test suite passes
  unchanged. Every difference is listed, with the test that pins it, in
  [DIVERGENCES.md](DIVERGENCES.md) (notably: `EventLoop::setDriver()` throws, the loop always
  runs on its native driver).
- **Native reactor:** epoll (Linux), kqueue (macOS), WSAPoll (Windows); no `stream_select()`
  limits, no ext-ev/event/uv needed, signals without ext-pcntl, precise timers.
- **Embeddable:** other Rust extensions can drive the same loop directly from Rust, see
  [EMBEDDING.md](EMBEDDING.md).
- **Pure Rust:** no C libraries. PHP 8.2–8.6, NTS and ZTS, Linux, macOS, Windows.

## Install

```bash
pie install danog/revolt-rs
```

Pre-built binaries for Linux, macOS and Windows; elsewhere [PIE](https://github.com/php/pie)
builds from source (Rust + libclang). Keep requiring revolt/event-loop (or amphp/amp) as before:
with the extension loaded, the PHP package is never used. danog/revolt-rs declares that it
provides revolt/event-loop, so a project that requires it through Composer
doesn't install the PHP package at all.

## Performance

Operations per second, **higher is better**: revolt-rs against the PHP library with each of its
drivers (Apple M3 Pro, PHP 8.5 NTS + opcache, release build, 5 alternating rounds, medians;
`tools/bench/run.sh` reproduces it).

| Case | **revolt-rs** | PHP StreamSelect | PHP Ev | PHP Uv | PHP Event | **revolt-rs vs fastest PHP** |
|---|---|---|---|---|---|---|
| `defer()`, 1M callbacks | **3.05 M/s** | 1.15 M/s | 1.13 M/s | 1.13 M/s | 1.12 M/s | **2.7× faster** |
| `queue()`, 1M microtasks | **7.20 M/s** | 3.51 M/s | 3.51 M/s | 3.51 M/s | 3.54 M/s | **2.0× faster** |
| `delay()`, 200k timers | **1.57 M/s** | 0.29 M/s | 0.79 M/s | 0.28 M/s | 0.70 M/s | **2.0× faster** |
| `repeat()`, 1000 timers × 200 | **4.80 M/s** | 0.58 M/s | 1.78 M/s | 1.13 M/s | 1.51 M/s | **2.7× faster** |
| `onReadable()` socket ping-pong | **0.43 M/s** | 0.27 M/s | 0.30 M/s | 0.31 M/s | 0.32 M/s | **1.3× faster** |
| `{main}` suspend/resume | **3.81 M/s** | 1.17 M/s | 1.17 M/s | 1.19 M/s | 1.17 M/s | **3.2× faster** |
| fiber suspend/resume | **2.89 M/s** | 1.32 M/s | 1.31 M/s | 1.30 M/s | 1.32 M/s | **2.2× faster** |

Every case also runs PHP closures, whose cost no loop can remove: what shrinks is the loop's own
overhead.

## Build and test

```bash
cargo build --release -p revolt-ext          # PHP / PHP_CONFIG select the PHP; Windows: nightly Rust
(cd tests && composer install) && (cd tools/phplib && composer install)
tools/test.sh                                # revolt/event-loop's suite + ours
REVOLT_RS_PHPLIB=1 tools/test.sh             # the same against the PHP library
tools/embed-test.sh                          # embedding tests
tools/asan.sh build && tools/asan.sh test    # AddressSanitizer
```

`tests/revolt` is revolt/event-loop's suite, unchanged ([tests/NOT_APPLICABLE.md](tests/NOT_APPLICABLE.md):
the one test that cannot apply); `tests/native` holds one test per divergence plus robustness
tests.

## License

MIT (see [LICENSE](LICENSE)). `tests/revolt` is revolt/event-loop's test suite, MIT, by its
authors.
