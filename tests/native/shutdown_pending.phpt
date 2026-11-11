--TEST--
Shutdown with pending callbacks, a suspended fiber and {main} never resumed: no crash, destructors run
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
        // the loop is still usable from destructors at shutdown
        EventLoop::defer(static function (): void {});
    }
}

[$a, $b] = stream_socket_pair(DIRECTORY_SEPARATOR === '\\' ? STREAM_PF_INET : STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
$timer = new Noisy('timer');
EventLoop::repeat(1, static function () use ($timer): void {});
$stream = new Noisy('stream');
EventLoop::onReadable($a, static function () use ($stream): void {});
$fiber = new Fiber(static function (): void {
    $local = new Noisy('fiber local');
    EventLoop::getSuspension()->suspend();
});
$fiber->start();
unset($timer, $stream);
echo "end of script\n";

?>
--EXPECT--
end of script
destroyed timer
destroyed stream
destroyed fiber local
