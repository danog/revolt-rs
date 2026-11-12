<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use Revolt\EventLoop\Driver\TracingDriver;
use Revolt\EventLoop\Driver\TracingDriverTest;

/** revolt/event-loop's TracingDriverTest, wrapping NativeDriver (DIVERGENCES.md "One driver class"). */
final class NativeTracingDriverTest extends TracingDriverTest
{
    public function getFactory(): callable
    {
        return static fn (): TracingDriver => new TracingDriver(Drivers::create());
    }
}
