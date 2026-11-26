<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;

/** A forked child keeps using its copy of the loop (the native poller is rebuilt there). */
final class ForkTest extends TestCase
{
    public function testChildUsesTheLoop(): void
    {
        if (!\function_exists('pcntl_fork')) {
            self::markTestSkipped('needs pcntl_fork()');
        }
        $driver = Drivers::create();
        [$a, $b] = \stream_socket_pair(\STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        [$c, $d] = \stream_socket_pair(\STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        // the parent's poller exists and watches $c before the fork
        $watch = $driver->onReadable($c, static function (): void {});
        $driver->delay(0, static function (): void {});
        $driver->unreference($watch);
        $driver->run();

        $pid = \pcntl_fork();
        if ($pid === 0) {
            $got = '';
            $driver->onReadable($a, static function (string $id) use ($a, $driver, &$got): void {
                $got = \fread($a, 4);
                $driver->cancel($id);
            });
            $driver->delay(2, static function () use ($driver): void { $driver->stop(); });
            $driver->run();
            // no shutdown functions or destructors of the parent's (PHPUnit's) objects
            \posix_kill(\getmypid(), $got === 'ping' ? \SIGUSR1 : \SIGUSR2);
            \sleep(5);
            exit(1);
        }
        // the parent's watcher of $c must not be shared with the child's poller
        $driver->cancel($watch);
        \fwrite($b, 'ping');
        \pcntl_waitpid($pid, $status);
        self::assertTrue(\pcntl_wifsignaled($status));
        self::assertSame(\SIGUSR1, \pcntl_wtermsig($status));
    }

    /**
     * fork() from inside the running loop with a change of the parent's still queued, as in
     * Psalm's worker pool (amphp/parallel's ForkContext): the IPC listener's watcher is disabled
     * by an accept() (queued until the next poll), the worker is forked, the parent re-enables
     * the watcher and applies that, and only then does the child poll. The child's first poll
     * must not apply the change it inherited: on Linux that was an EPOLL_CTL_DEL of the listener
     * on the parent's epoll instance (shared across fork()), after which the parent never saw a
     * connection again. The order is forced with blocking pipes, not timing; the parent's wait
     * is bounded by SIGALRM (a loop signal watcher: the timerfd was shared too).
     * Linux: fails without the child's rebuild (the parent's interest list loses the listener,
     * the parent never sees the pending connection). kqueue (macOS/BSD) is not inherited: the
     * parent is not at risk there, and without the rebuild the child's control calls fail on
     * the stale kqueue fd (its fds fall back to "always ready"), so this passes either way and
     * checks the semantics only: the parent accepts, the child's own watcher fires in the child.
     */
    public function testChildDoesNotApplyTheParentsQueuedChanges(): void
    {
        if (!\function_exists('pcntl_fork') || !\function_exists('pcntl_alarm')) {
            self::markTestSkipped('needs pcntl');
        }
        $driver = Drivers::create();
        $path = \sys_get_temp_dir() . '/revolt-rs-fork-' . \getmypid() . '.sock';
        @\unlink($path);
        $server = \stream_socket_server("unix://$path");
        \stream_set_blocking($server, false);
        // parent -> child: the parent applied its re-enable; child -> parent: the child polled
        // (applied what it inherited) and connected
        [$goRead, $goWrite] = \stream_socket_pair(\STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        [$doneRead, $doneWrite] = \stream_socket_pair(\STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        // the child's own watcher (never watched by the parent)
        [$childRead, $childWrite] = \stream_socket_pair(\STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        // (a broken run fails instead of blocking for ever)
        \stream_set_timeout($goRead, 10);
        \stream_set_timeout($doneRead, 10);
        $accepted = false;
        $pending = null;
        $inParentsPoller = null;
        $listener = $driver->onReadable($server, static function (string $id) use ($server, $driver, &$accepted): void {
            if (@\stream_socket_accept($server, 0) !== false) {
                $accepted = true;
                $driver->cancel($id);
            }
        });
        $pid = -1;
        $timedOut = false;
        $alarm = null;
        // a tick has registered the listener with the parent's poller when this runs
        $driver->defer(static function () use ($driver, $listener, $server, $path, $goRead, $goWrite, $doneRead, $doneWrite, $childRead, $childWrite, &$pid, &$pending, &$inParentsPoller, &$timedOut, &$alarm): void {
            // queued until the next poll, which comes after the fork
            $driver->disable($listener);
            $pid = \pcntl_fork();
            if ($pid === 0) {
                \fread($goRead, 1);
                // two ticks: the child has polled before it connects
                $driver->defer(static function () use ($driver, $path, $doneWrite, $childRead, $childWrite): void {
                    $driver->defer(static function () use ($driver, $path, $doneWrite, $childRead, $childWrite): void {
                        $client = \stream_socket_client("unix://$path");
                        \fwrite($doneWrite, '.');
                        $driver->onReadable($childRead, static function () use ($client): void {
                            // no shutdown functions or destructors of the parent's (PHPUnit's) objects
                            \posix_kill(\getmypid(), \SIGUSR1);
                        });
                        \fwrite($childWrite, '.');
                    });
                });
                return;
            }
            $driver->enable($listener);
            // the parent's wait is bounded by a signal watcher (a loop timer would not do: the
            // timerfd was shared too; a pcntl handler would not run while the loop spins on the
            // events of the child's fds); registered after the fork, so the child has no copy
            $alarm = $driver->onSignal(\SIGALRM, static function () use ($driver, &$timedOut): void {
                $timedOut = true;
                $driver->stop();
            });
            $driver->unreference($alarm);
            \pcntl_alarm(5);
            // the next tick applies the re-enable (before its deferred callbacks run)
            $driver->defer(static function () use ($server, $path, $goWrite, $doneRead, &$pending, &$inParentsPoller): void {
                \fwrite($goWrite, '.');
                \fread($doneRead, 1);
                $none = null;
                $read = [$server];
                $pending = \stream_select($read, $none, $none, 0) === 1;
                $inParentsPoller = self::inEpollInterestList($server);
            });
        });
        try {
            $driver->run();
        } finally {
            \pcntl_alarm(0);
            if ($alarm !== null) {
                $driver->cancel($alarm);
            }
            @\unlink($path);
            // the child ends itself once its own watcher fired
            for ($i = 0; $i < 500 && \pcntl_waitpid($pid, $status, \WNOHANG) === 0; $i++) {
                \usleep(10_000);
            }
            if ($i === 500) {
                \posix_kill($pid, \SIGKILL);
                \pcntl_waitpid($pid, $status);
            }
        }
        self::assertTrue($pending, 'no connection pending on the listener when the child is done');
        if ($inParentsPoller !== null) {
            self::assertTrue($inParentsPoller, "the child's first poll removed the listener from the parent's epoll instance");
        }
        self::assertFalse($timedOut, "a connection is pending on the listener, but the parent's loop never reported it readable");
        self::assertTrue($accepted);
        self::assertTrue(\pcntl_wifsignaled($status) && \pcntl_wtermsig($status) === \SIGUSR1, "the child's own watcher did not fire in the child");
    }

    /**
     * Linux: whether an epoll instance of this process has `$stream`'s file in its interest list
     * (/proc/self/fdinfo); null where that cannot be seen (no /proc; the PHP library's
     * stream_select driver has no epoll instance).
     */
    private static function inEpollInterestList($stream): ?bool
    {
        $ino = \fstat($stream)['ino'];
        $found = null;
        foreach (\glob('/proc/self/fd/*') ?: [] as $fd) {
            if (@\readlink($fd) !== 'anon_inode:[eventpoll]') {
                continue;
            }
            $found ??= false;
            $info = (string) @\file_get_contents('/proc/self/fdinfo/' . \basename($fd));
            \preg_match_all('/^tfd:.*\bino:\s*([0-9a-f]+)/m', $info, $m);
            foreach ($m[1] as $hex) {
                $found = $found || \hexdec($hex) === $ino;
            }
        }
        return $found;
    }

    /** The wait timer (Linux: a timerfd, shared across fork()) is the child's own there. */
    public function testChildTimerDoesNotMoveTheParents(): void
    {
        if (!\function_exists('pcntl_fork')) {
            self::markTestSkipped('needs pcntl_fork()');
        }
        $driver = Drivers::create();
        [$a, $b] = \stream_socket_pair(\STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        // the parent's poller and wait timer exist before the fork
        $watch = $driver->onReadable($a, static function (): void {});
        $driver->unreference($watch);
        $driver->delay(0.01, static function (): void {});
        $driver->run();

        $pid = \pcntl_fork();
        if ($pid === 0) {
            // armed after the parent's: a shared timer would now be due in 2 s for both
            \usleep(50_000);
            $driver->delay(2, static function () use ($driver): void { $driver->stop(); });
            $driver->run();
            \posix_kill(\getmypid(), \SIGUSR1);
            \sleep(5);
            exit(1);
        }
        $start = \hrtime(true);
        $driver->delay(0.2, static function (): void {});
        $driver->run();
        $elapsed = (\hrtime(true) - $start) / 1e9;
        \posix_kill($pid, \SIGKILL);
        \pcntl_waitpid($pid, $status);
        self::assertLessThan(1.0, $elapsed);
    }
}
