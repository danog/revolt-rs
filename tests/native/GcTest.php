<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop;
use Revolt\EventLoop\Driver\StreamSelectDriver;

/** Cycles through the native objects are collected. */
final class GcTest extends TestCase
{
    public function testDriverClosureCycles(): void
    {
        $refs = [];
        for ($i = 0; $i < 50; $i++) {
            $driver = new StreamSelectDriver();
            $driver->defer(static function () use ($driver): void {});
            $driver->delay(1, static function () use ($driver): void {});
            $driver->onReadable(\STDIN, static function () use ($driver): void {});
            $driver->setErrorHandler(static function () use ($driver): void {});
            $refs[] = \WeakReference::create($driver);
        }
        unset($driver);
        \gc_collect_cycles();
        foreach ($refs as $ref) {
            self::assertNull($ref->get());
        }
    }

    public function testSuspendedFiberInCycle(): void
    {
        $driver = new StreamSelectDriver();
        $fiber = new \Fiber(static function () use ($driver): void {
            $driver->getSuspension()->suspend();
        });
        $fiber->start();
        $ref = \WeakReference::create($fiber);
        $driverRef = \WeakReference::create($driver);
        unset($fiber, $driver);
        \gc_collect_cycles();
        self::assertNull($ref->get());
        self::assertNull($driverRef->get());
    }

    public function testQueuedArgumentsCycle(): void
    {
        $driver = new StreamSelectDriver();
        $object = new \stdClass();
        $object->driver = $driver;
        $driver->queue(static function (\stdClass $o): void {}, $object);
        $ref = \WeakReference::create($driver);
        unset($driver, $object);
        \gc_collect_cycles();
        self::assertNull($ref->get());
    }

    public function testFiberLocalCollected(): void
    {
        $local = new EventLoop\FiberLocal(static fn () => new \stdClass());
        $fiber = new \Fiber(static function () use ($local): void {
            $local->set(new \stdClass());
            \Fiber::suspend();
        });
        $fiber->start();
        $ref = \WeakReference::create($fiber);
        unset($fiber);
        \gc_collect_cycles();
        self::assertNull($ref->get());
    }
}
