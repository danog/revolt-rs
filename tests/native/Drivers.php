<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use Revolt\EventLoop\Driver;
use Revolt\EventLoop\Driver\NativeDriver;
use Revolt\EventLoop\Driver\StreamSelectDriver;

/**
 * The driver class the tests instantiate: revolt-rs's NativeDriver, or in the baseline run
 * against the PHP library (which has no NativeDriver) its StreamSelectDriver.
 */
final class Drivers
{
    /** @return class-string<Driver> */
    public static function class(): string
    {
        return \getenv('REVOLT_RS_PHPLIB') ? StreamSelectDriver::class : NativeDriver::class;
    }

    public static function create(): Driver
    {
        $class = self::class();
        return new $class();
    }
}
