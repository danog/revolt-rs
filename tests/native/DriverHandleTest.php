<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Driver\EvDriver;
use Revolt\EventLoop\Driver\EventDriver;
use Revolt\EventLoop\Driver\UvDriver;

/**
 * DIVERGENCES.md "All four driver classes share the native reactor": EvDriver, EventDriver and
 * UvDriver hand out their extension's loop object, but the native loop does not drive it.
 */
final class DriverHandleTest extends TestCase
{
    use NativeOnly;

    public function testEvHandleIsNotDriven(): void
    {
        self::nativeOnly();
        if (!EvDriver::isSupported()) {
            self::markTestSkipped('ext-ev is not loaded');
        }
        $driver = new EvDriver();
        $handle = $driver->getHandle();
        self::assertInstanceOf(\EvLoop::class, $handle);
        $fired = false;
        $watcher = $handle->timer(0, 0, static function () use (&$fired): void { $fired = true; });
        $driver->delay(0.02, static function (): void {});
        $driver->run();
        self::assertFalse($fired);
        $watcher->stop();
    }

    public function testEventHandleIsNotDriven(): void
    {
        self::nativeOnly();
        if (!EventDriver::isSupported()) {
            self::markTestSkipped('ext-event is not loaded');
        }
        $driver = new EventDriver();
        $handle = $driver->getHandle();
        self::assertInstanceOf(\EventBase::class, $handle);
        $fired = false;
        $event = \Event::timer($handle, static function () use (&$fired): void { $fired = true; });
        $event->addTimer(0);
        $driver->delay(0.02, static function (): void {});
        $driver->run();
        self::assertFalse($fired);
        $event->free();
    }

    public function testUvHandleIsNotDriven(): void
    {
        self::nativeOnly();
        if (!UvDriver::isSupported()) {
            self::markTestSkipped('ext-uv is not loaded');
        }
        $driver = new UvDriver();
        $handle = $driver->getHandle();
        self::assertInstanceOf(\UVLoop::class, $handle);
        $fired = false;
        $timer = \uv_timer_init($handle);
        \uv_timer_start($timer, 0, 0, static function () use (&$fired): void { $fired = true; });
        $driver->delay(0.02, static function (): void {});
        $driver->run();
        self::assertFalse($fired);
        \uv_close($timer);
    }
}
