<?php

// Run by EmbedderTest in a php process of its own: what revolt_embed_example does with the
// driver named by $argv[1], as JSON.

declare(strict_types=1);

use Revolt\EventLoop;
use Revolt\EventLoop\Driver;
use Revolt\EventLoop\Driver\StreamSelectDriver;
use Revolt\EventLoop\Driver\TracingDriver;
use Revolt\EventLoop\Internal\AbstractDriver;
use Revolt\EventLoop\Internal\DriverCallback;

if (getenv('REVOLT_EMBED_SCENARIO_PHPLIB')) {
    require __DIR__ . '/../../tools/phplib/vendor/autoload.php';
}

final class DelegatingDriver implements Driver
{
    public function __construct(private Driver $inner) {}
    public function run(): void { $this->inner->run(); }
    public function stop(): void { $this->inner->stop(); }
    public function getSuspension(): EventLoop\Suspension { return $this->inner->getSuspension(); }
    public function isRunning(): bool { return $this->inner->isRunning(); }
    public function queue(\Closure $closure, mixed ...$args): void { $this->inner->queue($closure, ...$args); }
    public function defer(\Closure $closure): string { return $this->inner->defer($closure); }
    public function delay(float $delay, \Closure $closure): string { return $this->inner->delay($delay, $closure); }
    public function repeat(float $interval, \Closure $closure): string { return $this->inner->repeat($interval, $closure); }
    public function onReadable(mixed $stream, \Closure $closure): string { return $this->inner->onReadable($stream, $closure); }
    public function onWritable(mixed $stream, \Closure $closure): string { return $this->inner->onWritable($stream, $closure); }
    public function onSignal(int $signal, \Closure $closure): string { return $this->inner->onSignal($signal, $closure); }
    public function enable(string $callbackId): string { return $this->inner->enable($callbackId); }
    public function cancel(string $callbackId): void { $this->inner->cancel($callbackId); }
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
}

/** A PHP subclass of AbstractDriver: timers only, on usleep(). */
final class SleepingDriver extends AbstractDriver
{
    /** @var array<string, EventLoop\Internal\TimerCallback> */
    private array $timers = [];

    protected function activate(array $callbacks): void
    {
        foreach ($callbacks as $callback) {
            if ($callback instanceof EventLoop\Internal\TimerCallback) {
                $this->timers[$callback->id] = $callback;
            }
        }
    }

    protected function dispatch(bool $blocking): void
    {
        if ($blocking && $this->timers !== []) {
            $next = min(array_map(static fn ($t) => $t->expiration, $this->timers));
            $wait = $next - $this->now();
            if ($wait > 0) {
                usleep((int) ($wait * 1e6));
            }
        }
        $now = $this->now();
        foreach ($this->timers as $id => $timer) {
            if ($timer->expiration <= $now) {
                if ($timer->repeat) {
                    $timer->expiration = $now + $timer->interval;
                } else {
                    unset($this->timers[$id]);
                }
                $this->enqueueCallback($timer);
            }
        }
    }

    protected function deactivate(DriverCallback $callback): void
    {
        unset($this->timers[$callback->id]);
    }

    public function getHandle(): mixed
    {
        return null;
    }

    protected function now(): float
    {
        return hrtime(true) / 1e9;
    }
}

$out = [];
switch ($argv[1] ?? 'default') {
    case 'default':
        break;
    case 'tracing':
        EventLoop::setDriver(new TracingDriver(new StreamSelectDriver()));
        break;
    case 'user-driver':
        EventLoop::setDriver(new DelegatingDriver(new StreamSelectDriver()));
        break;
    case 'subclass':
        EventLoop::setDriver(new SleepingDriver());
        break;
    case 'pending':
        // native callbacks still pending when their driver goes, and at the end of the script
        $before = revolt_embed_example_released();
        revolt_embed_example_delay(10);
        $fiber = new Fiber(static function (): void { revolt_embed_example_sleep(10); });
        $fiber->start();
        unset($fiber);
        EventLoop::setDriver(new StreamSelectDriver());
        gc_collect_cycles();
        $out = ['released_by_set_driver' => revolt_embed_example_released() - $before];
        revolt_embed_example_delay(10);
        $fiber = new Fiber(static function (): void { revolt_embed_example_sleep(10); });
        $fiber->start();
        echo json_encode($out), "\n";
        return;
    default:
        throw new \LogicException('unknown scenario');
}

$out['mode'] = revolt_embed_example_mode();
$out['owner'] = revolt_embed_example_owner();
$class = new ReflectionClass(EventLoop::class);
$out['internal_classes'] = $class->isInternal();
$out['classes_module'] = $class->getExtensionName() ?: null;
$out['driver'] = get_class(EventLoop::getDriver());

// {main} suspended on a timer
$start = hrtime(true);
$out['sleep_main'] = revolt_embed_example_sleep(0.05);
$out['slept_enough'] = hrtime(true) - $start >= 49_000_000;

// fibers suspended on timers, resumed in timer order
$order = [];
foreach ([0.03, 0.01, 0.02] as $i => $delay) {
    $fiber = new Fiber(static function () use ($i, $delay, &$order): void {
        $order[] = [$i, revolt_embed_example_sleep($delay)];
    });
    $fiber->start();
}
EventLoop::run();
$out['sleep_fibers'] = $order;

// PHP callbacks and the embedder's callbacks on the same loop
$log = [];
EventLoop::delay(0.02, static function () use (&$log): void { $log[] = 'php timer'; });
$fiber = new Fiber(static function () use (&$log): void { $log[] = 'embedder ' . revolt_embed_example_sleep(0.01); });
$fiber->start();
EventLoop::run();
$out['interleaved'] = $log;

if ($argv[1] !== 'subclass') {
    // a socket becoming readable
    [$a, $b] = stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    EventLoop::delay(0.01, static function () use ($b): void { fwrite($b, 'x'); });
    $out['wait_readable'] = revolt_embed_example_wait_readable($a);
    $out['read'] = fread($a, 1);
}

// the embedder's callback ids are Revolt callbacks of the loop
$id = revolt_embed_example_delay(10);
$out['native_id'] = $id === null ? null : [
    'type' => EventLoop::getType($id)->name,
    'listed' => in_array($id, EventLoop::getIdentifiers(), true),
    'referenced' => EventLoop::isReferenced($id),
];
if ($id !== null) {
    EventLoop::cancel($id);
}
$out['released'] = revolt_embed_example_released();

// an uncaught exception of the loop while {main} waits in the embedder: it is thrown from the
// embedder's function, whose timer is gone (last: {main}'s suspension is dead afterwards)
EventLoop::defer(static function (): void { throw new \RuntimeException('from a callback'); });
try {
    revolt_embed_example_sleep(10);
    $out['uncaught'] = 'not thrown';
} catch (EventLoop\UncaughtThrowable $e) {
    $out['uncaught'] = $e->getPrevious()->getMessage();
}
$out['released_after_uncaught'] = revolt_embed_example_released();
$out['identifiers_after_uncaught'] = EventLoop::getIdentifiers();

echo json_encode($out, JSON_PRETTY_PRINT), "\n";
