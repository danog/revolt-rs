<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\UnsupportedFeatureException;

/**
 * DIVERGENCES.md "Signals without ext-pcntl": signals are handled by the extension itself, and
 * the disposition a signal had before its first callback comes back after its last one.
 */
final class SignalTest extends TestCase
{
    use NativeOnly;

    private static function raise(int $signal): void
    {
        if (\function_exists('posix_kill')) {
            \posix_kill(\getmypid(), $signal);
        } else {
            \exec('kill -' . $signal . ' ' . \getmypid());
        }
    }

    public function testWindowsHasNoSignals(): void
    {
        if (\DIRECTORY_SEPARATOR !== '\\') {
            self::markTestSkipped('Windows only');
        }
        $this->expectException(UnsupportedFeatureException::class);
        $this->expectExceptionMessage('Signal handling requires the pcntl extension');
        (Drivers::create())->onSignal(2, static function (): void {});
    }

    /** NativeDriver handles signals whether ext-pcntl is loaded or not. */
    public function testSignalWithoutPcntlDependency(): void
    {
        if (\DIRECTORY_SEPARATOR === '\\') {
            self::markTestSkipped('Unix only');
        }
        if (!\extension_loaded('pcntl')) {
            // what is pinned: revolt/event-loop's StreamSelectDriver needs pcntl
            self::nativeOnly();
        }
        $sigusr2 = \defined('SIGUSR2') ? \SIGUSR2 : 31;
        $driver = Drivers::create();
        $got = [];
        $driver->onSignal($sigusr2, static function (string $id, int $signal) use (&$got, $driver): void {
            $got[] = $signal;
            $driver->cancel($id);
        });
        $driver->defer(static function () use ($sigusr2): void { self::raise($sigusr2); });
        $driver->run();
        self::assertSame([$sigusr2], $got);
    }

    /** The handler installed before (here pcntl's) is back after the last callback goes. */
    public function testPreviousDispositionRestored(): void
    {
        self::nativeOnly();
        if (!\extension_loaded('pcntl') || \DIRECTORY_SEPARATOR === '\\') {
            self::markTestSkipped('needs ext-pcntl');
        }
        $hits = [];
        \pcntl_signal(\SIGUSR1, static function () use (&$hits): void { $hits[] = 'pcntl'; });
        try {
            $driver = Drivers::create();
            $driver->onSignal(\SIGUSR1, static function (string $id) use (&$hits, $driver): void {
                $hits[] = 'loop';
                $driver->cancel($id);
            });
            $driver->defer(static function (): void { self::raise(\SIGUSR1); });
            $driver->run();
            self::raise(\SIGUSR1);
            \pcntl_signal_dispatch();
            self::assertSame(['loop', 'pcntl'], $hits);
        } finally {
            \pcntl_signal(\SIGUSR1, \SIG_DFL);
        }
    }
}
