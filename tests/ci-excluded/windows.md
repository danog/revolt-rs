# Tests excluded on the Windows CI runners

Applied by `tools/test.sh` only when `REVOLT_TEST_CI_EXCLUDE=windows` (set by the Windows jobs of
`.github/workflows/tests.yml`), for the extension and the PHP library alike.

PHP's `usleep()` on Windows waits on a waitable timer, which the GitHub Windows runners let expire
a few milliseconds early as measured by `microtime(true)`. The loop's own timers are not the cause
(the extension and the PHP library both time them with QueryPerformanceCounter, through
`hrtime()` for the library).

<!-- the list: one test per line, `- \`[Class]::method\`: reason` (read by tools/test.sh) -->

- `::testCorrectTimeoutIfBlockingBeforeDelay`: 0.5 s `usleep()` then a 1 s delay, lower bound
  1.5 s with no tolerance; run 43054345552 measured 1.4981 s (extension, PHP 8.3 ZTS) and
  1.4994 s (PHP library, 8.4 ZTS).
