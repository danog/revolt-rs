<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\UncaughtThrowable;

final class ErrorTest extends TestCase
{
    public function testExceptionFromErrorHandler(): void
    {
        $driver = new StreamSelectDriver();
        $driver->setErrorHandler(static function (\Throwable $e): void {
            throw new \LogicException('from handler', 0, $e);
        });
        $driver->defer(static function (): void { throw new \RuntimeException('from callback'); });
        try {
            $driver->run();
            self::fail('no exception');
        } catch (UncaughtThrowable $e) {
            self::assertStringStartsWith('Uncaught LogicException thrown in event loop error handler ', $e->getMessage());
            self::assertInstanceOf(\LogicException::class, $e->getPrevious());
            self::assertInstanceOf(\RuntimeException::class, $e->getPrevious()->getPrevious());
        }
        // the loop can be run again
        $ran = false;
        $driver->defer(static function () use (&$ran): void { $ran = true; });
        $driver->run();
        self::assertTrue($ran);
    }

    public function testNestedRun(): void
    {
        $driver = new StreamSelectDriver();
        $driver->defer(static function () use ($driver): void { $driver->run(); });
        try {
            $driver->run();
            self::fail('no exception');
        } catch (UncaughtThrowable $e) {
            self::assertInstanceOf(\Error::class, $e->getPrevious());
            self::assertSame('The event loop is already running', $e->getPrevious()->getMessage());
        }
    }

    public function testNestedRunInFiber(): void
    {
        $driver = new StreamSelectDriver();
        $fiber = new \Fiber(static function () use ($driver): void { $driver->run(); });
        $driver->defer(static function () use ($fiber): void { $fiber->start(); });
        try {
            $driver->run();
            self::fail('no exception');
        } catch (UncaughtThrowable $e) {
            self::assertSame('The event loop is already running', $e->getPrevious()->getMessage());
        }
    }

    public function testRunFromFiberIsRefused(): void
    {
        $driver = new StreamSelectDriver();
        $fiber = new \Fiber(static function () use ($driver): void { $driver->run(); });
        $this->expectException(\Error::class);
        $fiber->start();
    }
}
