<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Driver\TracingDriver;

final class TracingDriverDumpTest extends TestCase
{
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
}
