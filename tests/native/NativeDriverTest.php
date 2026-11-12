<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use Revolt\EventLoop;
use Revolt\EventLoop\Driver\NativeDriver;
use Revolt\EventLoop\Driver\StreamSelectDriverTest;
use Revolt\EventLoop\DriverFactory;
use Revolt\EventLoop\Internal\AbstractDriver;

/**
 * revolt/event-loop's driver suite (DriverTest, with StreamSelectDriverTest's own tests) against
 * NativeDriver; in the baseline run against the PHP library, against its StreamSelectDriver.
 *
 * DIVERGENCES.md "One driver class": NativeDriver instead of the library's four driver classes.
 */
final class NativeDriverTest extends StreamSelectDriverTest
{
    use NativeOnly;

    public function getFactory(): callable
    {
        return static fn () => Drivers::create();
    }

    public function testOnlyNativeDriver(): void
    {
        self::nativeOnly();
        $class = new \ReflectionClass(NativeDriver::class);
        self::assertTrue($class->isFinal());
        self::assertSame(AbstractDriver::class, $class->getParentClass()->getName());
        foreach (['StreamSelectDriver', 'EvDriver', 'EventDriver', 'UvDriver'] as $name) {
            self::assertFalse(\class_exists('Revolt\EventLoop\Driver\\' . $name, false), $name);
        }
        self::assertInstanceOf(NativeDriver::class, EventLoop::getDriver());
        self::assertInstanceOf(NativeDriver::class, (new DriverFactory())->create());
        self::assertNull((new NativeDriver())->getHandle());
    }
}
