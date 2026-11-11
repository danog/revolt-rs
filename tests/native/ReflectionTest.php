<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

use PHPUnit\Framework\TestCase;
use Revolt\EventLoop;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Internal\AbstractDriver;

/** DIVERGENCES.md "Reflection". */
final class ReflectionTest extends TestCase
{
    use NativeOnly;

    public function testClassesAreInternal(): void
    {
        self::nativeOnly();
        $class = new \ReflectionClass(StreamSelectDriver::class);
        self::assertTrue($class->isInternal());
        self::assertFalse($class->getFileName());
        self::assertFalse($class->getDocComment());
        self::assertTrue($class->isCloneable());
        $run = new \ReflectionMethod(AbstractDriver::class, 'run');
        self::assertTrue($run->isInternal());
        self::assertSame([], $run->getAttributes());
    }

    public function testPrivateStateStaysAtDefaults(): void
    {
        self::nativeOnly();
        $driver = new StreamSelectDriver();
        [$read] = \stream_socket_pair(\DIRECTORY_SEPARATOR === '\\' ? \STREAM_PF_INET : \STREAM_PF_UNIX, \STREAM_SOCK_STREAM, \STREAM_IPPROTO_IP);
        $id = $driver->onReadable($read, static function (): void {});
        $property = new \ReflectionProperty(StreamSelectDriver::class, 'readStreams');
        self::assertSame([], $property->getValue($driver));
        $driver->cancel($id);
    }

    public function testPrivateMethodsDoNothing(): void
    {
        self::nativeOnly();
        $driver = new StreamSelectDriver();
        $ran = false;
        $driver->defer(static function () use (&$ran): void { $ran = true; });
        $isEmpty = new \ReflectionMethod(AbstractDriver::class, 'isEmpty');
        self::assertFalse($isEmpty->invoke($driver));
        (new \ReflectionMethod(AbstractDriver::class, 'tick'))->invoke($driver, false);
        (new \ReflectionMethod(AbstractDriver::class, 'invokeCallbacks'))->invoke($driver);
        self::assertFalse($ran);
        $driver->run();
        self::assertTrue($ran);
        self::assertTrue($isEmpty->invoke($driver));
    }

    public function testEventLoopDriverPropertyIsReal(): void
    {
        $driver = EventLoop::getDriver();
        self::assertSame($driver, (new \ReflectionProperty(EventLoop::class, 'driver'))->getValue());
    }
}
