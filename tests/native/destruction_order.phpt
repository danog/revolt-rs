--TEST--
The callback fiber stays alive as long as the loop lives (Revolt issue #105, on the loop's own driver)
--SKIPIF--
<?php

if (PHP_VERSION_ID < 80400) {
    echo 'skip PHP 8.4+ required';
}

?>
--FILE--
<?php

use Revolt\EventLoop;

require __DIR__ . '/../bootstrap.php';

final class a {
    private static self $a;
    public static function getInstance(): self {
        return self::$a ??= new self;
    }

    public function __destruct()
    {
        echo "Destroying ", self::class, "\n";
        $suspension = EventLoop::getSuspension();
        EventLoop::delay(1.0, $suspension->resume(...));
        $suspension->suspend();
        echo "Finished " . self::class, "\n";
    }
}

EventLoop::defer(function () {
    echo "start\n";
});

a::getInstance();

EventLoop::run();

?>
--EXPECT--
start
Destroying a
Finished a
