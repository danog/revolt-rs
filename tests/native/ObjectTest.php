<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Driver\TracingDriver;
use Revolt\EventLoop\Internal\DriverSuspension;

/** DIVERGENCES.md "Cloning", "new DriverSuspension(...)", "Extra arguments". */
final class ObjectTest extends TestCase
{
    use NativeOnly;

    public static function uncloneable(): iterable
    {
        yield 'driver' => [static fn () => new StreamSelectDriver(), StreamSelectDriver::class];
        yield 'tracing driver' => [static fn () => new TracingDriver(new StreamSelectDriver()), TracingDriver::class];
        yield 'suspension' => [static fn () => (new StreamSelectDriver())->getSuspension(), DriverSuspension::class];
    }

    /** @dataProvider uncloneable */
    public function testCloneIsRefused(\Closure $make, string $class): void
    {
        self::nativeOnly();
        $object = $make();
        $this->expectException(\Error::class);
        $this->expectExceptionMessage('Trying to clone an uncloneable object of class ' . $class);
        clone $object;
    }

    public function testSuspensionsComeFromTheDriver(): void
    {
        self::nativeOnly();
        $this->expectException(\Error::class);
        $this->expectExceptionMessage('created by the event loop driver');
        $noop = static function (): void {};
        new DriverSuspension($noop, $noop, $noop, new \WeakMap());
    }

    public function testInternalArgumentErrors(): void
    {
        self::nativeOnly();
        try {
            EventLoop::defer();
            self::fail('no error');
        } catch (\ArgumentCountError $e) {
            self::assertSame('Revolt\EventLoop::defer() expects exactly 1 argument, 0 given', $e->getMessage());
        }
        try {
            EventLoop::delay('soon', static function (): void {});
            self::fail('no error');
        } catch (\TypeError $e) {
            self::assertSame('Revolt\EventLoop::delay(): Argument #1 ($delay) must be of type float, string given', $e->getMessage());
        }
        // extra arguments are ignored
        $id = EventLoop::defer(static function (): void {}, 'extra');
        EventLoop::cancel($id);
    }
}
