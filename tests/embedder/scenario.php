<?php

// Run by EmbedderTest in a php process of its own: what revolt_embed_example does with the
// driver named by $argv[1], as JSON.

declare(strict_types=1);

use Revolt\EventLoop;
use Revolt\EventLoop\Driver\StreamSelectDriver;

if (getenv('REVOLT_EMBED_SCENARIO_PHPLIB')) {
    require __DIR__ . '/../../tools/phplib/vendor/autoload.php';
}

$out = [];
switch ($argv[1] ?? 'default') {
    case 'default':
        break;
    case 'pending':
        // native callbacks and waits still pending at the end of the script (freed at shutdown)
        revolt_embed_example_delay(10);
        $fiber = new Fiber(static function (): void { revolt_embed_example_sleep(10); });
        $fiber->start();
        [$a, $b] = stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
        $fiber2 = new Fiber(static function () use ($a): void { revolt_embed_example_src_wait($a); });
        $fiber2->start();
        echo json_encode(['pending' => true]), "\n";
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

if (true) {
    // a socket becoming readable
    [$a, $b] = stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    EventLoop::delay(0.01, static function () use ($b): void { fwrite($b, 'x'); });
    $out['wait_readable'] = revolt_embed_example_wait_readable($a);
    $out['read'] = fread($a, 1);
}

// the idle check: nothing enabled (referenced or not), queued or armed
$idle = [];
$idle['start'] = revolt_embed_example_idle();
$id = EventLoop::delay(10, static function (): void {});
$idle['timer'] = revolt_embed_example_idle();
EventLoop::unreference($id);
$idle['unreferenced timer'] = revolt_embed_example_idle();
EventLoop::disable($id);
$idle['disabled timer'] = revolt_embed_example_idle();
EventLoop::cancel($id);
EventLoop::defer(static function (): void {});
$idle['defer'] = revolt_embed_example_idle();
EventLoop::run();
EventLoop::queue(static function (): void {});
$idle['microtask'] = revolt_embed_example_idle();
EventLoop::run();
$idle['after run'] = revolt_embed_example_idle();
[$ia, $ib] = stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
$fiber = new Fiber(static function () use ($ia): void { revolt_embed_example_src_wait($ia); });
$fiber->start();
$idle['fiber waiting on a source'] = revolt_embed_example_idle();
fwrite($ib, 'i');
EventLoop::run();
$idle['source done'] = revolt_embed_example_idle();
revolt_embed_example_src_forget($ia);
if (DIRECTORY_SEPARATOR !== '\\' && $idle['start'] !== null) {
    // (the PHP library's drivers need pcntl for signals)
    $sig = EventLoop::onSignal(PHP_OS_FAMILY === 'Darwin' ? 31 : 12, static function (): void {});
    EventLoop::unreference($sig);
    $idle['signal watcher'] = revolt_embed_example_idle();
    EventLoop::cancel($sig);
}
$out['idle'] = $idle;

// sources: sockets waited for edge-triggered, outside the callback records
$pair = static fn () => stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
[$a, $b] = $pair();
EventLoop::delay(0.01, static function () use ($b): void { fwrite($b, 'x'); });
$out['src_wait'] = revolt_embed_example_src_wait($a);
$out['src_read'] = fread($a, 1);
// a {main} wait a flush ended leaves the loop where a callback's resume would: what is queued
// afterwards runs at the next run()
$queued = false;
EventLoop::queue(static function () use (&$queued): void { $queued = true; });
EventLoop::run();
$out['queued_after_main_wait'] = $queued;
// readiness the loop saw while nobody waited ends the next wait at once
fwrite($b, 'y');
EventLoop::delay(0.01, static function (): void {});
EventLoop::run();
$out['src_ready'] = revolt_embed_example_src_wait($a);
$out['src_read_ready'] = fread($a, 1);
// several fibers resumed by one poll: one flush
$stats = revolt_embed_example_src_stats();
$pairs = [$pair(), $pair(), $pair()];
$results = [];
foreach ($pairs as $i => [$r]) {
    (new Fiber(static function () use ($i, $r, &$results): void { $results[$i] = revolt_embed_example_src_wait($r); }))->start();
}
EventLoop::delay(0.01, static function () use ($pairs): void {
    foreach ($pairs as [, $w]) {
        fwrite($w, 'z');
    }
});
EventLoop::run();
ksort($results);
$out['src_fibers'] = $results;
$after = revolt_embed_example_src_stats();
$out['src_batch'] = ['takes' => $after['takes'] - $stats['takes'], 'flushes' => $after['flushes'] - $stats['flushes']];
// writable
$out['src_writable'] = revolt_embed_example_src_wait($a, true);
foreach (array_merge([[$a, $b]], $pairs) as [$r]) {
    revolt_embed_example_src_forget($r);
}
// a socket with the number of a forgotten one is registered anew
fclose($a);
fclose($b);
[$a, $b] = $pair();
EventLoop::delay(0.01, static function () use ($b): void { fwrite($b, 'w'); });
$out['src_reused'] = revolt_embed_example_src_wait($a);

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

// the loop's driver cannot be replaced (the PHP library's can)
$driver = EventLoop::getDriver();
try {
    EventLoop::setDriver(new StreamSelectDriver());
    $out['set_driver'] = 'not thrown';
} catch (EventLoop\UnsupportedFeatureException|\Error $e) {
    $out['set_driver'] = $e->getMessage();
}
$out['same_driver'] = $driver === EventLoop::getDriver();

echo json_encode($out, JSON_PRETTY_PRINT), "\n";
