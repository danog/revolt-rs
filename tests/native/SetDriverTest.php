<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop;
use Revolt\EventLoop\Driver\NativeDriver;
use Revolt\EventLoop\Driver\TracingDriver;
use Revolt\EventLoop\DriverFactory;
use Revolt\EventLoop\UnsupportedFeatureException;

/**
 * DIVERGENCES.md "The loop's driver cannot be replaced". A fresh process per test: the loop's
 * driver is made from the environment on first use.
 *
 * @runTestsInSeparateProcesses
 * @preserveGlobalState disabled
 */
final class SetDriverTest extends TestCase
{
    use NativeOnly;

    private const REFUSED = 'Revolt\EventLoop::setDriver() is not supported: the event loop always runs on its native driver';

    public function testSetDriverIsRefused(): void
    {
        self::nativeOnly();
        $driver = EventLoop::getDriver();
        $inner = new NativeDriver();
        foreach ([new NativeDriver(), new TracingDriver($inner)] as $other) {
            try {
                EventLoop::setDriver($other);
                self::fail('setDriver() did not throw');
            } catch (UnsupportedFeatureException $e) {
                self::assertSame(self::REFUSED, $e->getMessage());
            }
            self::assertSame($driver, EventLoop::getDriver());
        }
        // the argument is checked first, as for any internal function
        $this->expectException(\TypeError::class);
        EventLoop::setDriver(new \stdClass());
    }

    /** Writing EventLoop::$driver does not change the loop's driver either. */
    public function testDriverPropertyIsNotTheLoop(): void
    {
        self::nativeOnly();
        $driver = EventLoop::getDriver();
        $other = new TracingDriver(new NativeDriver());
        (new \ReflectionProperty(EventLoop::class, 'driver'))->setValue(null, $other);
        self::assertSame($driver, EventLoop::getDriver());
        $ran = false;
        EventLoop::defer(static function () use (&$ran): void { $ran = true; });
        EventLoop::run();
        self::assertTrue($ran);
    }

    /**
     * REVOLT_DRIVER selects nothing (revolt-rs has one driver class, DIVERGENCES.md "One driver
     * class"); REVOLT_DRIVER_DEBUG_TRACE: the loop ignores it (DriverFactory::create() still
     * honours it).
     */
    public function testLoopDriverIgnoresOtherDriverSettings(): void
    {
        self::nativeOnly();
        \putenv('REVOLT_DRIVER=NoSuchDriver');
        \putenv('REVOLT_DRIVER_DEBUG_TRACE=1');
        try {
            self::assertInstanceOf(NativeDriver::class, EventLoop::getDriver());
            $traced = (new DriverFactory())->create();
            self::assertInstanceOf(TracingDriver::class, $traced);
            \putenv('REVOLT_DRIVER_DEBUG_TRACE');
            self::assertInstanceOf(NativeDriver::class, (new DriverFactory())->create());
        } finally {
            \putenv('REVOLT_DRIVER');
            \putenv('REVOLT_DRIVER_DEBUG_TRACE');
        }
    }

    /** REVOLT_DRIVER naming one of revolt/event-loop's driver classes selects nothing either. */
    public function testLoopDriverIgnoresDriverClassNames(): void
    {
        self::nativeOnly();
        \putenv('REVOLT_DRIVER=Revolt\EventLoop\Driver\EvDriver');
        try {
            self::assertInstanceOf(NativeDriver::class, EventLoop::getDriver());
            self::assertInstanceOf(NativeDriver::class, (new DriverFactory())->create());
        } finally {
            \putenv('REVOLT_DRIVER');
        }
    }
}
