# Tests excluded on the macOS CI runners

Applied by `tools/test.sh` only when `REVOLT_TEST_CI_EXCLUDE=macos` (set by the macOS jobs of
`.github/workflows/tests.yml`), for the extension and the PHP library alike. Local runs on macOS
keep them: they pass on a Mac whose timers are not coalesced.

The GitHub macOS runners coalesce the timeouts of sleeping processes by tens of milliseconds: the
workflow's timer step measured `usleep()` overshooting by +14 to +73 ms (`usleep(0.01)` +28.6 ms,
`usleep(0.10)` +72.5 ms, `usleep(0.50)` +13.9 ms; with `taskpolicy -a` +41.1, +33.1, +52.7 ms:
no better). The tests below `usleep(500000)` inside a callback and allow 100 ms in all, so they
fail on those runners with the PHP library too (run 43042104542: up to 1.737 s for a 1.6 s bound,
1.153 s for 1.1 s; StreamSelectDriver and TracingDriver, extension and library).

<!-- the list: one test per line, `- \`[Class]::method\`: reason` (read by tools/test.sh) -->

- `::testCorrectTimeoutIfBlockingBeforeDelay`: 0.5 s `usleep()` then a 1 s delay, bound 1.6 s;
  measured 1.61–1.74 s on the runners.
- `::testCorrectTimeoutIfBlockingBeforeActivate`: a 1 s delay then a 0.5 s `usleep()`, bound
  1.1 s; measured 1.100–1.153 s on the runners.
