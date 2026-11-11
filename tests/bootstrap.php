<?php
// The suites run against the extension with the PHP library absent. This guard fails loudly if
// anything ever asks the autoloader for one of revolt/event-loop's own classes (the extension
// registers them at MINIT, so it never should). REVOLT_RS_PHPLIB=1 runs the suites against the
// PHP library instead (tools/phplib), without the extension, as a baseline.
require __DIR__ . '/vendor/autoload.php';

if (getenv('REVOLT_RS_PHPLIB')) {
    if (extension_loaded('revolt')) {
        fwrite(STDERR, "REVOLT_RS_PHPLIB=1 needs PHP without the revolt extension\n");
        exit(1);
    }
    require __DIR__ . '/../tools/phplib/vendor/autoload.php';
    return;
}

if (!extension_loaded('revolt') && !class_exists('Revolt\EventLoop', false)) {
    fwrite(STDERR, "the revolt extension is not loaded\n");
    exit(1);
}

$api = json_decode(file_get_contents(__DIR__ . '/../tools/api-1.0.9.json'), true)['classes'];
spl_autoload_register(static function (string $class) use ($api): void {
    if (isset($api[$class])) {
        throw new \LogicException("revolt/event-loop's class $class was autoloaded: the extension must provide it");
    }
}, true, true);
