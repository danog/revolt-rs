# Carried tests that cannot apply

The tests of revolt/event-loop 1.0.9 (`tests/revolt`) run unchanged against the extension.
The tests listed here are excluded by `tools/test.sh` (and CI), each with the reason. Every one
of them stands for a deliberate difference documented in `DIVERGENCES.md` and pinned by a test
of our own in `tests/native`.

<!-- the list: one test per line, `- \`Class::method\`: reason` (read by tools/test.sh) -->

- `Revolt\EventLoop\Driver\StreamSelectDriverTest::testTooLargeFileDescriptorSet`: expects the
  FD_SETSIZE error of stream_select() with 2002 descriptors; the native reactor (epoll/kqueue/
  WSAPoll) has no such limit, so nothing is thrown and the test's never-readable watchers keep
  the loop running forever. Pinned by `tests/native/NoFdSetSizeLimitTest.php`.
