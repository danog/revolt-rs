<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop\Internal\AbstractDriver;
use Revolt\EventLoop\Internal\DriverCallback;
use Revolt\EventLoop\Internal\TimerCallback;

/** A PHP subclass of AbstractDriver: its hooks and overrides are what the loop calls. */
final class UserlandDriverTest extends TestCase
{
    use NativeOnly;

    private static function driver(bool $callParentConstructor = true): AbstractDriver
    {
        return new class ($callParentConstructor) extends AbstractDriver {
            public array $log = [];
            /** @var array<string, DriverCallback> */
            public array $active = [];
            public float $time = 100.0;

            public function __construct(bool $callParentConstructor)
            {
                if ($callParentConstructor) {
                    parent::__construct();
                }
            }

            protected function activate(array $callbacks): void
            {
                foreach ($callbacks as $callback) {
                    $this->log[] = 'activate ' . $callback->id . ' ' . (new \ReflectionClass($callback))->getShortName();
                    $this->active[$callback->id] = $callback;
                }
            }

            protected function dispatch(bool $blocking): void
            {
                $this->log[] = 'dispatch ' . ($blocking ? 'blocking' : 'non-blocking');
                $this->time += 10;
                foreach ($this->active as $callback) {
                    if ($callback instanceof TimerCallback && $callback->expiration <= $this->time) {
                        if (!$callback->repeat) {
                            $this->deactivate($callback);
                        } else {
                            $callback->expiration = $this->time + $callback->interval;
                        }
                        $this->enqueueCallback($callback);
                    }
                }
            }

            protected function deactivate(DriverCallback $callback): void
            {
                $this->log[] = 'deactivate ' . $callback->id;
                unset($this->active[$callback->id]);
            }

            public function getHandle(): mixed
            {
                return 'handle';
            }

            protected function now(): float
            {
                return $this->time;
            }

            public function cancel(string $callbackId): void
            {
                $this->log[] = 'cancel ' . $callbackId;
                parent::cancel($callbackId);
            }
        };
    }

    public function testHooks(): void
    {
        $driver = self::driver();
        $driver->defer(static function () use ($driver): void { $driver->log[] = 'defer ran'; });
        $driver->delay(5, static function () use ($driver): void { $driver->log[] = 'timer ran'; });
        $repeat = $driver->repeat(15, static function (string $id) use ($driver): void {
            $driver->log[] = 'repeat ran';
            $driver->cancel($id);
        });
        self::assertSame('handle', $driver->getHandle());
        $driver->run();
        self::assertSame([
            'activate b TimerCallback',
            'activate c TimerCallback',
            'dispatch non-blocking',
            'deactivate b',
            'cancel a', // a fired defer is cancelled through $this->cancel()
            'defer ran',
            'cancel b', // so is a fired delay
            'deactivate b',
            'timer ran',
            'dispatch non-blocking',
            'deactivate c',
            'repeat ran',
            'cancel ' . $repeat,
        ], $driver->log);
        self::assertSame([], $driver->getIdentifiers());
    }

    public function testStreamCallbackObjects(): void
    {
        $driver = self::driver();
        [$a, $b] = \stream_socket_pair(\DIRECTORY_SEPARATOR === '\\' ? \STREAM_PF_INET : \STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        $id = $driver->onReadable($a, static function (): void {});
        $driver->defer(static function () use ($driver, $id): void { $driver->cancel($id); });
        $driver->run();
        self::assertSame(['activate a StreamReadableCallback', 'dispatch non-blocking', 'cancel b', 'cancel a', 'deactivate a'], $driver->log);
    }

    /**
     * DIVERGENCES.md "A PHP subclass of AbstractDriver": without parent::__construct() it works.
     */
    public function testWithoutParentConstructor(): void
    {
        self::nativeOnly();
        $driver = self::driver(false);
        $ran = false;
        $driver->defer(static function () use (&$ran): void { $ran = true; });
        $driver->run();
        self::assertTrue($ran);
    }

    /**
     * DIVERGENCES.md "A PHP subclass of AbstractDriver": writes to a DriverCallback's public
     * properties change that object only.
     */
    public function testCallbackPropertyWritesAreLocal(): void
    {
        self::nativeOnly();
        $driver = self::driver();
        $id = $driver->delay(5, static function (): void {});
        $driver->defer(static function () use ($driver, $id): void {
            foreach ($driver->active as $callback) {
                $callback->referenced = false;
            }
            $driver->log[] = 'referenced ' . ($driver->isReferenced($id) ? 'yes' : 'no');
        });
        $driver->run();
        self::assertContains('referenced yes', $driver->log);
    }
}
