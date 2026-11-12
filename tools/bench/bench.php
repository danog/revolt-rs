<?php

// Event loop micro-benchmarks: the same code against the extension or the PHP library.
//   php bench.php [driver] [case...]     driver: default | ev | uv | event (PHP library: which
//                                        driver class EventLoop uses; ignored by the extension,
//                                        whose drivers all share the native reactor)
//                                        case: a name, "readme" (the README's cases, the default)
//                                        or "all" (every case the loaded code can run)
// Prints one line per case: "<case> <operations per second>". The library is loaded from
// tools/phplib when no extension registered the Revolt classes. BENCH_SCALE (default 1) scales
// every case's operation count (e.g. 0.01 under valgrind). The embed-* cases need the example
// embedder (examples/embedder) loaded.

declare(strict_types=1);

use Revolt\EventLoop;
use Revolt\EventLoop\FiberLocal;

if (!class_exists(EventLoop::class, false)) {
    require __DIR__ . '/../phplib/vendor/autoload.php';
}

const README_CASES = ['defer', 'queue', 'delay', 'repeat', 'readable', 'suspend-main', 'suspend-fiber'];

$driver = $argv[1] ?? 'default';
$cases = array_slice($argv, 2) ?: ['readme'];
$native = !(new ReflectionClass(EventLoop::class))->isUserDefined();
$scale = (float) (getenv('BENCH_SCALE') ?: 1);
if (!$native) {
    match ($driver) {
        'default' => EventLoop::setDriver(new EventLoop\Driver\StreamSelectDriver()),
        'ev' => EventLoop::setDriver(new EventLoop\Driver\EvDriver()),
        'uv' => EventLoop::setDriver(new EventLoop\Driver\UvDriver()),
        'event' => EventLoop::setDriver(new EventLoop\Driver\EventDriver()),
        default => (function () use ($driver) { fwrite(STDERR, "unknown driver $driver\n"); exit(2); })(),
    };
}

/** @return float operations per second of $run(), which performs $n operations */
function measure(int $n, Closure $run): float
{
    global $scale;
    $n = max(1, (int) ($n * $scale));
    $start = hrtime(true);
    $run($n);
    return $n / ((hrtime(true) - $start) / 1e9);
}

/** @return list{resource, resource} a non-blocking connected socket pair */
function socket_pair(): array
{
    $p = stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    stream_set_blocking($p[0], false);
    stream_set_blocking($p[1], false);
    return $p;
}

/** {main}, or a fiber started from a microtask, runs $body to the end */
function in_fiber(Closure $body): void
{
    $fiber = new Fiber($body);
    EventLoop::queue($fiber->start(...));
    EventLoop::run();
}

$all = [
    // 1M defer callbacks, scheduled up front, run in one tick
    'defer' => static fn () => measure(1_000_000, static function (int $n): void {
        $count = 0;
        $callback = static function () use (&$count): void { $count++; };
        for ($i = 0; $i < $n; $i++) {
            EventLoop::defer($callback);
        }
        EventLoop::run();
        assert($count === $n);
    }),
    // the same with an empty closure (the least PHP work per callback)
    'defer-empty' => static fn () => measure(1_000_000, static function (int $n): void {
        $callback = static function (): void {};
        for ($i = 0; $i < $n; $i++) {
            EventLoop::defer($callback);
        }
        EventLoop::run();
    }),
    // 1M defer() + cancel() pairs, never run: a callback's creation and destruction
    'defer-cancel' => static fn () => measure(1_000_000, static function (int $n): void {
        $callback = static function (): void {};
        for ($i = 0; $i < $n; $i++) {
            EventLoop::cancel(EventLoop::defer($callback));
        }
    }),
    // 1M microtasks with an argument
    'queue' => static fn () => measure(1_000_000, static function (int $n): void {
        $count = 0;
        $callback = static function (int $i) use (&$count): void { $count++; };
        for ($i = 0; $i < $n; $i++) {
            EventLoop::queue($callback, $i);
        }
        EventLoop::run();
        assert($count === $n);
    }),
    // 1M microtasks without arguments, empty closure
    'queue-empty' => static fn () => measure(1_000_000, static function (int $n): void {
        $callback = static function (): void {};
        for ($i = 0; $i < $n; $i++) {
            EventLoop::queue($callback);
        }
        EventLoop::run();
    }),
    // 200k one-shot timers with random delays of 0-10 ms
    'delay' => static fn () => measure(200_000, static function (int $n): void {
        mt_srand(1);
        $count = 0;
        $callback = static function () use (&$count): void { $count++; };
        for ($i = 0; $i < $n; $i++) {
            EventLoop::delay(mt_rand(0, 10) / 1000, $callback);
        }
        EventLoop::run();
        assert($count === $n);
    }),
    // 200k timers with random deadlines (1-100 s) activated, then all cancelled: the timer heap's
    // insertion and removal at random positions
    'delay-cancel' => static fn () => measure(200_000, static function (int $n): void {
        mt_srand(1);
        $callback = static function (): void {};
        $ids = [];
        for ($i = 0; $i < $n; $i++) {
            $ids[] = EventLoop::delay(mt_rand(1000, 100_000) / 1000, $callback);
        }
        EventLoop::defer(static function () use (&$ids): void {
            foreach ($ids as $id) {
                EventLoop::cancel($id);
            }
        });
        EventLoop::run();
    }),
    // 1000 repeat timers at 0 s, 200 rounds each, then cancelled
    'repeat' => static fn () => measure(200_000, static function (int $n): void {
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
    // 500k disable() + enable() pairs of an active repeat timer (from a callback)
    'enable-disable' => static fn () => measure(500_000, static function (int $n): void {
        $id = EventLoop::repeat(1000, static function (): void {});
        EventLoop::defer(static function () use ($id, $n): void {
            for ($i = 0; $i < $n; $i++) {
                EventLoop::disable($id);
                EventLoop::enable($id);
            }
            EventLoop::cancel($id);
        });
        EventLoop::run();
    }),
    // 1M unreference() + reference() pairs
    'reference' => static fn () => measure(1_000_000, static function (int $n): void {
        $id = EventLoop::defer(static function (): void {});
        for ($i = 0; $i < $n; $i++) {
            EventLoop::unreference($id);
            EventLoop::reference($id);
        }
        EventLoop::cancel($id);
    }),
    // ping-pong of one byte over a socket pair: 100k round trips (200k readable callbacks)
    'readable' => static fn () => measure(100_000, static function (int $n): void {
        [$a, $b] = socket_pair();
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
    // the same ping-pong with 1000 more readable callbacks on 100 idle socket pairs
    'readable-many' => static fn () => measure(100_000, static function (int $n): void {
        $idle = [];
        $ids = [];
        for ($p = 0; $p < 100; $p++) {
            $idle[] = $pair = socket_pair();
            for ($w = 0; $w < 10; $w++) {
                $ids[] = EventLoop::onReadable($pair[0], static function (): void {});
            }
        }
        [$a, $b] = socket_pair();
        $trips = 0;
        $echo = EventLoop::onReadable($b, static function () use ($b): void {
            fwrite($b, fread($b, 1));
        });
        EventLoop::onReadable($a, static function (string $id) use ($a, $n, $echo, &$trips, &$ids): void {
            fread($a, 1);
            if (++$trips === $n) {
                EventLoop::cancel($id);
                EventLoop::cancel($echo);
                foreach ($ids as $i) {
                    EventLoop::cancel($i);
                }
                return;
            }
            fwrite($a, 'x');
        });
        fwrite($a, 'x');
        EventLoop::run();
    }),
    // a writable callback on an always-writable socket: 200k invocations (one per tick)
    'writable' => static fn () => measure(200_000, static function (int $n): void {
        [$a, $b] = socket_pair();
        $count = 0;
        EventLoop::onWritable($a, static function (string $id) use (&$count, $n): void {
            if (++$count === $n) {
                EventLoop::cancel($id);
            }
        });
        EventLoop::run();
    }),
    // 200k onReadable() watchers activated for one tick, then cancelled (from {main})
    'watch-cancel' => static fn () => measure(200_000, static function (int $n): void {
        [$a, $b] = socket_pair();
        $callback = static function (): void {};
        $suspension = EventLoop::getSuspension();
        $resume = static function () use (&$suspension): void { $suspension->resume(); };
        for ($i = 0; $i < $n; $i++) {
            $id = EventLoop::onReadable($a, $callback);
            EventLoop::defer($resume);
            $suspension->suspend();
            EventLoop::cancel($id);
        }
    }),
    // {main} suspended and resumed by a microtask: 200k round trips
    'suspend-main' => static fn () => measure(200_000, static function (int $n): void {
        for ($i = 0; $i < $n; $i++) {
            $suspension = EventLoop::getSuspension();
            EventLoop::queue($suspension->resume(...), $i);
            $suspension->suspend();
        }
    }),
    // a fiber suspended and resumed by a microtask: 200k round trips
    'suspend-fiber' => static fn () => measure(200_000, static function (int $n): void {
        in_fiber(static function () use ($n): void {
            for ($i = 0; $i < $n; $i++) {
                $suspension = EventLoop::getSuspension();
                EventLoop::queue($suspension->resume(...), $i);
                $suspension->suspend();
            }
        });
    }),
    // {main} resumed from a defer callback (one closure for every round trip)
    'suspend-main-defer' => static fn () => measure(200_000, static function (int $n): void {
        $suspension = EventLoop::getSuspension();
        $resume = static function () use (&$suspension): void { $suspension->resume(); };
        for ($i = 0; $i < $n; $i++) {
            EventLoop::defer($resume);
            $suspension->suspend();
        }
    }),
    // a fiber resumed from a defer callback
    'suspend-fiber-defer' => static fn () => measure(200_000, static function (int $n): void {
        in_fiber(static function () use ($n): void {
            $suspension = EventLoop::getSuspension();
            $resume = static function () use (&$suspension): void { $suspension->resume(); };
            for ($i = 0; $i < $n; $i++) {
                EventLoop::defer($resume);
                $suspension->suspend();
            }
        });
    }),
    // a fiber resumed with throw() from a defer callback
    'throw-fiber' => static fn () => measure(200_000, static function (int $n): void {
        in_fiber(static function () use ($n): void {
            $suspension = EventLoop::getSuspension();
            $e = new Exception('x');
            $throw = static function () use (&$suspension, $e): void { $suspension->throw($e); };
            for ($i = 0; $i < $n; $i++) {
                EventLoop::defer($throw);
                try {
                    $suspension->suspend();
                } catch (Exception) {
                }
            }
        });
    }),
    // {main} resumed with throw() from a defer callback
    'throw-main' => static fn () => measure(200_000, static function (int $n): void {
        $suspension = EventLoop::getSuspension();
        $e = new Exception('x');
        $throw = static function () use (&$suspension, $e): void { $suspension->throw($e); };
        for ($i = 0; $i < $n; $i++) {
            EventLoop::defer($throw);
            try {
                $suspension->suspend();
            } catch (Exception) {
            }
        }
    }),
    // 1M getSuspension() calls in a fiber
    'get-suspension' => static fn () => measure(1_000_000, static function (int $n): void {
        in_fiber(static function () use ($n): void {
            for ($i = 0; $i < $n; $i++) {
                EventLoop::getSuspension();
            }
        });
    }),
    // 1M FiberLocal::get() in a fiber
    'fiberlocal-get' => static fn () => measure(1_000_000, static function (int $n): void {
        in_fiber(static function () use ($n): void {
            $local = new FiberLocal(static fn (): int => 1);
            for ($i = 0; $i < $n; $i++) {
                $local->get();
            }
        });
    }),
    // 1M FiberLocal::set() in a fiber
    'fiberlocal-set' => static fn () => measure(1_000_000, static function (int $n): void {
        in_fiber(static function () use ($n): void {
            $local = new FiberLocal(static fn (): int => 1);
            for ($i = 0; $i < $n; $i++) {
                $local->set($i);
            }
        });
    }),
    // 100k defer callbacks throwing, each to the error handler
    'error-handler' => static fn () => measure(100_000, static function (int $n): void {
        $e = new Exception('x');
        $count = 0;
        EventLoop::setErrorHandler(static function (Throwable $t) use (&$count): void { $count++; });
        $callback = static function () use ($e): void { throw $e; };
        for ($i = 0; $i < $n; $i++) {
            EventLoop::defer($callback);
        }
        EventLoop::run();
        EventLoop::setErrorHandler(null);
        assert($count === $n);
    }),
    // the example embedder: native timer waits of {main} (delay(0), suspend, resume)
    'embed-sleep-main' => static fn () => measure(200_000, static function (int $n): void {
        for ($i = 0; $i < $n; $i++) {
            revolt_embed_example_sleep(0);
        }
    }),
    // the same in a fiber
    'embed-sleep-fiber' => static fn () => measure(200_000, static function (int $n): void {
        in_fiber(static function () use ($n): void {
            for ($i = 0; $i < $n; $i++) {
                revolt_embed_example_sleep(0);
            }
        });
    }),
    // native readable waits of {main} on a socket with data (a callback per wait)
    'embed-readable' => static fn () => measure(200_000, static function (int $n): void {
        [$a, $b] = socket_pair();
        for ($i = 0; $i < $n; $i++) {
            fwrite($a, 'x');
            revolt_embed_example_wait_readable($b);
            fread($b, 1);
        }
    }),
    // source waits of a fiber, resumed by the batched flush: 200k one-byte round trips
    'embed-src' => static fn () => measure(200_000, static function (int $n): void {
        [$a, $b] = socket_pair();
        in_fiber(static function () use ($n, $a, $b): void {
            for ($i = 0; $i < $n; $i++) {
                fwrite($a, 'x');
                revolt_embed_example_src_wait($b);
                fread($b, 1);
            }
            revolt_embed_example_src_forget($b);
        });
    }),
    // 200k native timers that do nothing, run
    'embed-delay' => static fn () => measure(200_000, static function (int $n): void {
        for ($i = 0; $i < $n; $i++) {
            revolt_embed_example_delay(0);
        }
        EventLoop::run();
    }),
];

$expand = [];
foreach ($cases as $c) {
    $expand = [...$expand, ...match ($c) {
        'readme' => README_CASES,
        'all' => array_values(array_filter(array_keys($all), static fn (string $k): bool => !str_starts_with($k, 'embed-') || function_exists('revolt_embed_example_sleep'))),
        default => [$c],
    }];
}
$results = [];
foreach ($expand as $case) {
    if (!isset($all[$case])) {
        throw new LogicException("unknown case $case");
    }
    gc_collect_cycles();
    $results[$case] = $all[$case]();
}
foreach ($results as $case => $ops) {
    printf("%s %.0f\n", $case, $ops);
}
