<?php

// Event loop micro-benchmarks: the same code against the extension or the PHP library.
//   php bench.php [driver] [case...]     driver: default | ev | uv | event (PHP library: which
//                                        driver class EventLoop uses; ignored by the extension,
//                                        whose drivers all share the native reactor)
// Prints one line per case: "<case> <operations per second>". The library is loaded from
// tools/phplib when the extension is not.

declare(strict_types=1);

use Revolt\EventLoop;

if (!extension_loaded('revolt')) {
    require __DIR__ . '/../phplib/vendor/autoload.php';
}

$driver = $argv[1] ?? 'default';
$cases = array_slice($argv, 2) ?: ['defer', 'queue', 'delay', 'repeat', 'readable', 'suspend-main', 'suspend-fiber'];
switch ($driver) {
    case 'default':
        EventLoop::setDriver(new EventLoop\Driver\StreamSelectDriver());
        break;
    case 'ev':
        EventLoop::setDriver(new EventLoop\Driver\EvDriver());
        break;
    case 'uv':
        EventLoop::setDriver(new EventLoop\Driver\UvDriver());
        break;
    case 'event':
        EventLoop::setDriver(new EventLoop\Driver\EventDriver());
        break;
    default:
        fwrite(STDERR, "unknown driver $driver\n");
        exit(2);
}

/** @return float operations per second of $run(), which performs $n operations */
function measure(int $n, Closure $run): float
{
    $start = hrtime(true);
    $run($n);
    return $n / ((hrtime(true) - $start) / 1e9);
}

$results = [];
foreach ($cases as $case) {
    gc_collect_cycles();
    $results[$case] = match ($case) {
        // 1M defer callbacks, scheduled up front, run in one tick
        'defer' => measure(1_000_000, static function (int $n): void {
            $count = 0;
            $callback = static function () use (&$count): void { $count++; };
            for ($i = 0; $i < $n; $i++) {
                EventLoop::defer($callback);
            }
            EventLoop::run();
            assert($count === $n);
        }),
        // 1M microtasks with an argument
        'queue' => measure(1_000_000, static function (int $n): void {
            $count = 0;
            $callback = static function (int $i) use (&$count): void { $count++; };
            for ($i = 0; $i < $n; $i++) {
                EventLoop::queue($callback, $i);
            }
            EventLoop::run();
            assert($count === $n);
        }),
        // 200k one-shot timers with random delays of 0-10 ms
        'delay' => measure(200_000, static function (int $n): void {
            mt_srand(1);
            $count = 0;
            $callback = static function () use (&$count): void { $count++; };
            for ($i = 0; $i < $n; $i++) {
                EventLoop::delay(mt_rand(0, 10) / 1000, $callback);
            }
            EventLoop::run();
            assert($count === $n);
        }),
        // 1000 repeat timers at 0 s, 200 rounds each, then cancelled
        'repeat' => measure(200_000, static function (int $n): void {
            $count = 0;
            for ($t = 0; $t < 1000; $t++) {
                EventLoop::repeat(0, static function (string $id) use (&$count, $n): void {
                    if (++$count > $n - 1000) {
                        EventLoop::cancel($id);
                    }
                });
            }
            EventLoop::run();
        }),
        // ping-pong of one byte over a socket pair: 100k round trips (200k readable callbacks)
        'readable' => measure(100_000, static function (int $n): void {
            [$a, $b] = stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
            stream_set_blocking($a, false);
            stream_set_blocking($b, false);
            $trips = 0;
            $echo = EventLoop::onReadable($b, static function () use ($b): void {
                fwrite($b, fread($b, 1));
            });
            EventLoop::onReadable($a, static function (string $id) use ($a, $n, $echo, &$trips): void {
                fread($a, 1);
                if (++$trips === $n) {
                    EventLoop::cancel($id);
                    EventLoop::cancel($echo);
                    return;
                }
                fwrite($a, 'x');
            });
            fwrite($a, 'x');
            EventLoop::run();
            fclose($a);
            fclose($b);
        }),
        // {main} suspended and resumed by a microtask: 200k round trips
        'suspend-main' => measure(200_000, static function (int $n): void {
            for ($i = 0; $i < $n; $i++) {
                $suspension = EventLoop::getSuspension();
                EventLoop::queue($suspension->resume(...), $i);
                $suspension->suspend();
            }
        }),
        // a fiber suspended and resumed by a microtask: 200k round trips
        'suspend-fiber' => measure(200_000, static function (int $n): void {
            $fiber = new Fiber(static function () use ($n): void {
                for ($i = 0; $i < $n; $i++) {
                    $suspension = EventLoop::getSuspension();
                    EventLoop::queue($suspension->resume(...), $i);
                    $suspension->suspend();
                }
            });
            EventLoop::queue($fiber->start(...));
            EventLoop::run();
        }),
        default => throw new LogicException("unknown case $case"),
    };
}
foreach ($results as $case => $ops) {
    printf("%s %.0f\n", $case, $ops);
}
