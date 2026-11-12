# revolt-rs

[revolt/event-loop](https://github.com/revoltphp/event-loop) as a native PHP extension, written
in Rust. Install it, and everything built on Revolt (Amp v3, your own code) runs unchanged,
1.4–16× faster than the PHP library on its default `stream_select()` driver.

- **Drop-in:** the same classes and API; revolt/event-loop's own test suite passes. The few
  differences are listed in [DIVERGENCES.md](DIVERGENCES.md).
- **No extra extensions:** no `stream_select()` limits, and no ext-ev, ext-event, ext-uv or
  ext-pcntl needed for fast I/O, timers and signals.
- PHP 8.2–8.6, NTS and ZTS, on Linux, macOS and Windows.

## Install

```bash
pie install danog/revolt-rs
```

[PIE](https://github.com/php/pie) installs a pre-built binary, or builds from source (Rust +
libclang) where none matches. Keep requiring revolt/event-loop (or amphp/amp) as before: with
the extension loaded, the PHP package is never used.

## Performance

Operations per second, **higher is better**: revolt-rs against the PHP library with each of its
drivers (Apple M3 Pro, PHP 8.5 NTS + opcache, release build, 5 alternating rounds, medians;
`tools/bench/run.sh` reproduces it). The speed-up is against StreamSelect, the driver most
installs run: the PHP library picks Ev, Uv or Event only when that extension is installed.

| Case | **revolt-rs** | PHP StreamSelect | PHP Ev | PHP Uv | PHP Event | **revolt-rs vs PHP StreamSelect** |
|---|---|---|---|---|---|---|
| `defer()`, 1M callbacks | **3.99 M/s** | 1.07 M/s | 1.08 M/s | 1.06 M/s | 1.03 M/s | **3.7× faster** |
| `queue()`, 1M microtasks | **16.99 M/s** | 3.33 M/s | 3.41 M/s | 3.44 M/s | 3.32 M/s | **5.1× faster** |
| `delay()`, 200k timers | **2.59 M/s** | 0.24 M/s | 0.77 M/s | 0.26 M/s | 0.65 M/s | **10.8× faster** |
| `repeat()`, 1000 timers × 200 | **9.03 M/s** | 0.56 M/s | 1.76 M/s | 1.09 M/s | 1.43 M/s | **16.1× faster** |
| `onReadable()` socket ping-pong | **0.49 M/s** | 0.27 M/s | 0.30 M/s | 0.30 M/s | 0.31 M/s | **1.8× faster** |
| `{main}` suspend/resume | **5.06 M/s** | 1.17 M/s | 1.14 M/s | 1.16 M/s | 1.13 M/s | **4.3× faster** |
| fiber suspend/resume | **5.10 M/s** | 1.25 M/s | 1.27 M/s | 1.31 M/s | 1.31 M/s | **4.1× faster** |

Linux (AMD Ryzen 9 5950X, PHP 8.5 NTS + opcache, the same rounds; the PHP library with
StreamSelect, the driver available there):

| Case | **revolt-rs** | PHP StreamSelect | **revolt-rs vs PHP StreamSelect** |
|---|---|---|---|
| `defer()`, 1M callbacks | **2.21 M/s** | 0.77 M/s | **2.9× faster** |
| `queue()`, 1M microtasks | **10.35 M/s** | 1.83 M/s | **5.7× faster** |
| `delay()`, 200k timers | **2.10 M/s** | 0.21 M/s | **9.9× faster** |
| `repeat()`, 1000 timers × 200 | **6.74 M/s** | 0.53 M/s | **12.8× faster** |
| `onReadable()` socket ping-pong | **0.19 M/s** | 0.13 M/s | **1.4× faster** |
| `{main}` suspend/resume | **4.03 M/s** | 0.90 M/s | **4.5× faster** |
| fiber suspend/resume | **4.01 M/s** | 0.94 M/s | **4.3× faster** |

Every case also runs PHP closures, whose cost no loop can remove: what shrinks is the loop's own
overhead. `php tools/bench/bench.php default all` has a case for every hot path.

## Development

```bash
cargo build --release -p revolt-ext
(cd tests && composer install) && tools/test.sh
```

Other Rust extensions can run on the same loop: see [EMBEDDING.md](EMBEDDING.md).

## License

MIT (see [LICENSE](LICENSE)). `tests/revolt` is revolt/event-loop's test suite, MIT, by its
authors.
