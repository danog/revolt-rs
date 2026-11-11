<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Driver\StreamSelectDriver;

/** DIVERGENCES.md "Equal timer expirations": they fire in creation order. */
final class TimerOrderTest extends TestCase
{
    use NativeOnly;

    public function testEqualExpirationsFireInCreationOrder(): void
    {
        self::nativeOnly();
        $driver = new StreamSelectDriver();
        $order = [];
        // one now() for all: create them inside a single callback, with the same delay
        $driver->defer(static function () use ($driver, &$order): void {
            for ($i = 0; $i < 200; $i++) {
                $driver->delay(0.005, static function () use ($i, &$order): void { $order[] = $i; });
            }
        });
        $driver->run();
        self::assertSame(\range(0, 199), $order);
    }
}
