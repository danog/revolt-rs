<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Driver\TracingDriver;
use Revolt\EventLoop\UncaughtThrowable;

final class TracingDriverDumpTest extends TestCase
{
    use NativeOnly;

    public function testDump(): void
    {
        $driver = new TracingDriver(new StreamSelectDriver());
        $id = $driver->delay(1, static function (): void {});
        $unreferenced = $driver->unreference($driver->delay(1, static function (): void {}));
        $dump = $driver->dump();
        self::assertStringStartsWith("Enabled, referenced callbacks keeping the loop running: Callback identifier: $id\r\n", $dump);
        self::assertStringContainsString('TracingDriver->delay()', $dump);
        self::assertStringNotContainsString("Callback identifier: $unreferenced", $dump);
        $driver->cancel($id);
        $driver->cancel($unreferenced);
        self::assertSame('Enabled, referenced callbacks keeping the loop running:', $driver->dump());
    }

    /**
     * DIVERGENCES.md "TracingDriver's wrapper closures": named without a file and line.
     */
    public function testWrapperClosureName(): void
    {
        self::nativeOnly();
        $driver = new TracingDriver(new StreamSelectDriver());
        $driver->defer(static function (): void {
            throw new \RuntimeException('boom');
        });
        try {
            $driver->run();
            self::fail('no exception');
        } catch (UncaughtThrowable $e) {
            self::assertSame(
                'Uncaught RuntimeException thrown in event loop callback Revolt\EventLoop\Driver\TracingDriver::{closure}; use Revolt\EventLoop::setErrorHandler() to gracefully handle such exceptions: boom',
                $e->getMessage(),
            );
        }
    }

    /**
     * DIVERGENCES.md "TracingDriver's wrapper closures": the creation trace of a callback
     * created through the facade has no EventLoop.php frame.
     */
    public function testDumpThroughFacadeHasNoEventLoopFrame(): void
    {
        self::nativeOnly();
        $previous = EventLoop::getDriver();
        $driver = new TracingDriver(new StreamSelectDriver());
        EventLoop::setDriver($driver);
        try {
            $id = EventLoop::delay(1, static function (): void {});
            $dump = $driver->dump();
            EventLoop::cancel($id);
        } finally {
            EventLoop::setDriver($previous);
        }
        self::assertStringContainsString("Callback identifier: $id", $dump);
        self::assertStringContainsString('EventLoop::delay()', $dump);
        self::assertStringNotContainsString('EventLoop.php', $dump);
    }
}
