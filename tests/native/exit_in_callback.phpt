--TEST--
exit() inside an event loop callback ends the script: shutdown functions and destructors run
--FILE--
<?php

use Revolt\EventLoop;

require __DIR__ . '/../bootstrap.php';

final class Noisy
{
    public function __construct(private string $name)
    {
    }

    public function __destruct()
    {
        echo "destroyed {$this->name}\n";
    }
}

register_shutdown_function(static function (): void {
    echo "shutdown function\n";
});

$held = new Noisy('held by a pending callback');
EventLoop::delay(10, static function () use ($held): void {});
EventLoop::defer(static function (): void {
    $fiber = new Fiber(static function (): void {
        $local = new Noisy('local of a suspended fiber');
        EventLoop::getSuspension()->suspend();
    });
    $fiber->start();
    echo "exiting\n";
    exit(0);
});
EventLoop::run();
echo "not reached\n";

?>
--EXPECT--
exiting
shutdown function
destroyed held by a pending callback
destroyed local of a suspended fiber
