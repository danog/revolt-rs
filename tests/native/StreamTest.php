<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Driver;
use Revolt\EventLoop\Driver\EvDriver;
use Revolt\EventLoop\Driver\EventDriver;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Driver\UvDriver;

final class StreamTest extends TestCase
{
    use NativeOnly;

    public static function drivers(): iterable
    {
        foreach ([StreamSelectDriver::class, EvDriver::class, EventDriver::class, UvDriver::class] as $class) {
            yield $class => [$class];
        }
    }

    private static function pair(): array
    {
        $domain = \DIRECTORY_SEPARATOR === '\\' ? \STREAM_PF_INET : \STREAM_PF_UNIX;
        return \stream_socket_pair($domain, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
    }

    /**
     * DIVERGENCES.md "Data in PHP's stream read buffer": every driver class reports a stream
     * readable while PHP's buffer holds data, though the socket itself is drained.
     *
     * @dataProvider drivers
     */
    public function testBufferedDataIsReadable(string $class): void
    {
        if ($class !== StreamSelectDriver::class) {
            self::nativeOnly();
        }
        if (\method_exists($class, 'isSupported') && !$class::isSupported()) {
            self::markTestSkipped($class . ' is not supported');
        }
        /** @var Driver $driver */
        $driver = new $class();
        [$a, $b] = self::pair();
        \fwrite($b, "one\ntwo\n");
        \usleep(10000);
        self::assertSame("one\n", \fgets($a)); // PHP reads both lines into its buffer
        $got = null;
        $timeout = $driver->delay(1, static function () use (&$got): void { $got = 'timeout'; });
        $driver->onReadable($a, static function (string $id) use ($a, $driver, $timeout, &$got): void {
            $got = \fgets($a);
            $driver->cancel($id);
            $driver->cancel($timeout);
        });
        $driver->run();
        self::assertSame("two\n", $got);
    }

    /**
     * DIVERGENCES.md "Streams that are not streams": a stream closed before its callback is
     * activated is a TypeError from the loop.
     */
    public function testClosedStreamIsTypeError(): void
    {
        self::nativeOnly();
        $driver = new StreamSelectDriver();
        [$a, $b] = self::pair();
        $driver->onReadable($a, static function (): void {});
        \fclose($a);
        $this->expectException(\TypeError::class);
        $this->expectExceptionMessage('Invalid stream for callback a');
        $driver->run();
    }
}
