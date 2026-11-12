<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Driver\StreamSelectDriver;

/** A forked child keeps using its copy of the loop (the native poller is rebuilt there). */
final class ForkTest extends TestCase
{
    public function testChildUsesTheLoop(): void
    {
        if (!\function_exists('pcntl_fork')) {
            self::markTestSkipped('needs pcntl_fork()');
        }
        $driver = new StreamSelectDriver();
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

    /** The wait timer (Linux: a timerfd, shared across fork()) is the child's own there. */
    public function testChildTimerDoesNotMoveTheParents(): void
    {
        if (!\function_exists('pcntl_fork')) {
            self::markTestSkipped('needs pcntl_fork()');
        }
        $driver = new StreamSelectDriver();
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
