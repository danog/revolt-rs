<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop;
use Revolt\EventLoop\Driver;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Driver\TracingDriver;

/**
 * The global driver must be swappable: a fresh process (the suites leave it running).
 *
 * @runTestsInSeparateProcesses
 * @preserveGlobalState disabled
 */
final class SetDriverTest extends TestCase
{
    use NativeOnly;

    protected function tearDown(): void
    {
        EventLoop::setDriver(new StreamSelectDriver());
    }

    /**
     * DIVERGENCES.md "setDriver()'s temporary driver": destructors run by setDriver()'s
     * gc_collect_cycles() see an AbstractDriver@anonymous without a file name.
     */
    public function testTemporaryDriverClass(): void
    {
        self::nativeOnly();
        $seen = [];
        $cycle = new class ($seen) {
            public ?object $self = null;

            public function __construct(private array &$seen)
            {
            }

            public function __destruct()
            {
                $this->seen[] = \get_class(EventLoop::getDriver());
            }
        };
        $cycle->self = $cycle;
        unset($cycle);
        EventLoop::setDriver(new StreamSelectDriver());
        self::assertSame(['Revolt\EventLoop\Internal\AbstractDriver@anonymous'], $seen);
    }

    /** A user implementation of Driver set with setDriver() is called through PHP. */
    public function testUserDriverImplementation(): void
    {
        $inner = new StreamSelectDriver();
        $log = [];
        $driver = new class ($inner, $log) implements Driver {
            public function __construct(private Driver $inner, private array &$log)
            {
            }

            public function run(): void { $this->log[] = 'run'; $this->inner->run(); }
            public function stop(): void { $this->inner->stop(); }
            public function getSuspension(): EventLoop\Suspension { return $this->inner->getSuspension(); }
            public function isRunning(): bool { return $this->inner->isRunning(); }
            public function queue(\Closure $closure, mixed ...$args): void { $this->log[] = 'queue'; $this->inner->queue($closure, ...$args); }
            public function defer(\Closure $closure): string { $this->log[] = 'defer'; return $this->inner->defer($closure); }
            public function delay(float $delay, \Closure $closure): string { $this->log[] = 'delay'; return $this->inner->delay($delay, $closure); }
            public function repeat(float $interval, \Closure $closure): string { return $this->inner->repeat($interval, $closure); }
            public function onReadable(mixed $stream, \Closure $closure): string { return $this->inner->onReadable($stream, $closure); }
            public function onWritable(mixed $stream, \Closure $closure): string { return $this->inner->onWritable($stream, $closure); }
            public function onSignal(int $signal, \Closure $closure): string { return $this->inner->onSignal($signal, $closure); }
            public function enable(string $callbackId): string { return $this->inner->enable($callbackId); }
            public function cancel(string $callbackId): void { $this->log[] = 'cancel'; $this->inner->cancel($callbackId); }
            public function disable(string $callbackId): string { return $this->inner->disable($callbackId); }
            public function reference(string $callbackId): string { return $this->inner->reference($callbackId); }
            public function unreference(string $callbackId): string { return $this->inner->unreference($callbackId); }
            public function setErrorHandler(?\Closure $errorHandler): void { $this->inner->setErrorHandler($errorHandler); }
            public function getErrorHandler(): ?\Closure { return $this->inner->getErrorHandler(); }
            public function getHandle(): mixed { return $this->inner->getHandle(); }
            public function getIdentifiers(): array { return $this->inner->getIdentifiers(); }
            public function getType(string $callbackId): EventLoop\CallbackType { return $this->inner->getType($callbackId); }
            public function isEnabled(string $callbackId): bool { return $this->inner->isEnabled($callbackId); }
            public function isReferenced(string $callbackId): bool { return $this->inner->isReferenced($callbackId); }
            public function __debugInfo(): array { return $this->inner->__debugInfo(); }
        };
        EventLoop::setDriver($driver);
        self::assertSame($driver, EventLoop::getDriver());
        $order = [];
        EventLoop::queue(static function () use (&$order): void { $order[] = 'queued'; });
        EventLoop::defer(static function () use (&$order): void { $order[] = 'deferred'; });
        $id = EventLoop::delay(5, static function (): void {});
        EventLoop::cancel($id);
        EventLoop::delay(0.001, static function () use (&$order): void { $order[] = 'delayed'; });
        EventLoop::run();
        self::assertSame(['queued', 'deferred', 'delayed'], $order);
        self::assertSame(['queue', 'defer', 'delay', 'cancel', 'delay', 'run'], $log);
    }

    /**
     * DIVERGENCES.md "TracingDriver's wrapper closures": the creation trace of a callback
     * created through the facade has no EventLoop.php frame.
     */
    public function testDumpThroughFacadeHasNoEventLoopFrame(): void
    {
        self::nativeOnly();
        $driver = new TracingDriver(new StreamSelectDriver());
        EventLoop::setDriver($driver);
        $id = EventLoop::delay(1, static function (): void {});
        $dump = $driver->dump();
        EventLoop::cancel($id);
        self::assertStringContainsString("Callback identifier: $id", $dump);
        self::assertStringContainsString('EventLoop::delay()', $dump);
        self::assertStringNotContainsString('EventLoop.php', $dump);
    }
}
