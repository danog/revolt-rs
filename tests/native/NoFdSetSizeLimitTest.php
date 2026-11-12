<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;

/**
 * DIVERGENCES.md "No FD_SETSIZE limit": NativeDriver watches more than 1024 descriptors
 * (revolt/event-loop throws "You have reached the limits of stream_select()").
 */
final class NoFdSetSizeLimitTest extends TestCase
{
    use NativeOnly;

    public function testMoreThanFdSetSizeStreams(): void
    {
        self::nativeOnly();
        if (\DIRECTORY_SEPARATOR === '\\') {
            self::markTestSkipped('socket pairs of this size are a Unix test');
        }
        $loop = Drivers::create();
        $sockets = [];
        for ($i = 0; $i < 1001; $i++) {
            $sockets[] = \stream_socket_pair(\STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        }
        $ids = [];
        foreach ($sockets as [$left, $right]) {
            $ids[] = $loop->onReadable($left, static function (): void {});
            $ids[] = $loop->onReadable($right, static function (): void {});
        }
        // the last pair becomes readable: its callback runs and stops the loop
        [$a, $b] = $sockets[1000];
        \fwrite($b, 'x');
        $read = 0;
        $loop->onReadable($a, function (string $id) use ($loop, &$read, $ids): void {
            $read++;
            $loop->cancel($id);
            foreach ($ids as $other) {
                $loop->cancel($other);
            }
        });
        $loop->run();
        self::assertSame(1, $read);
        foreach ($sockets as [$left, $right]) {
            \fclose($left);
            \fclose($right);
        }
    }
}
