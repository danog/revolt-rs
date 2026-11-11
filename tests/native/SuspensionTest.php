<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Internal\AbstractDriver;
use Revolt\EventLoop\Internal\DriverCallback;
use Revolt\EventLoop\UncaughtThrowable;

final class SuspensionTest extends TestCase
{
    use NativeOnly;

    /**
     * DIVERGENCES.md "A suspension whose fiber was destroyed": a resume() from the fiber's own
     * unwinding (it was collected while suspended) is ignored and the loop goes on.
     */
    public function testResumeOfDestroyedFiberIsIgnored(): void
    {
        $driver = new StreamSelectDriver();
        $suspension = null;
        $finally = false;
        $fiber = new \Fiber(static function () use (&$suspension, &$finally, $driver): void {
            $suspension = $driver->getSuspension();
            try {
                $suspension->suspend();
            } finally {
                $finally = true;
                $suspension->resume('late');
            }
        });
        $fiber->start();
        unset($fiber, $suspension);
        \gc_collect_cycles();
        self::assertTrue($finally);
        $ran = false;
        $driver->defer(static function () use (&$ran): void { $ran = true; });
        $driver->run();
        self::assertTrue($ran);
    }

    /**
     * DIVERGENCES.md "{main} suspend() while the loop is executing": from a userland driver's
     * dispatch() hook.
     */
    public function testMainSuspendDuringDispatch(): void
    {
        self::nativeOnly();
        $driver = new class () extends AbstractDriver {
            private int $dispatches = 0;

            protected function activate(array $callbacks): void
            {
            }

            protected function dispatch(bool $blocking): void
            {
                if ($this->dispatches++ === 0) {
                    $this->getSuspension()->suspend();
                }
            }

            protected function deactivate(DriverCallback $callback): void
            {
            }

            public function getHandle(): mixed
            {
                return null;
            }

            protected function now(): float
            {
                return \hrtime(true) / 1e9;
            }
        };
        $driver->delay(0.001, static function (): void {});
        $this->expectException(\Error::class);
        $this->expectExceptionMessage('The event loop is already running');
        $driver->run();
    }

    /**
     * DIVERGENCES.md "Interrupt descriptions": an exception out of a fiber resumed by a
     * suspension names DriverSuspension::resume.
     */
    public function testUncaughtFromResumedFiberNamesResume(): void
    {
        self::nativeOnly();
        $driver = new StreamSelectDriver();
        $suspension = null;
        $fiber = new \Fiber(static function () use (&$suspension, $driver): void {
            $suspension = $driver->getSuspension();
            $suspension->suspend();
            throw new \RuntimeException('boom');
        });
        $fiber->start();
        $driver->defer(static function () use (&$suspension): void { $suspension->resume(); });
        try {
            $driver->run();
            self::fail('no exception');
        } catch (UncaughtThrowable $e) {
            self::assertSame(
                'Uncaught RuntimeException thrown in event loop callback Revolt\EventLoop\Internal\DriverSuspension::resume; use Revolt\EventLoop::setErrorHandler() to gracefully handle such exceptions: boom',
                $e->getMessage(),
            );
        }
    }
}
