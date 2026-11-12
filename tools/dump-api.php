<?php
// Dumps the public surface of revolt/event-loop through Reflection, as JSON: every class,
// interface and enum of the Revolt\ namespace with its modifiers, parent, interfaces,
// constants (values), enum cases, properties (types, modifiers, defaults) and methods
// (modifiers, return types, parameters with names, types, defaults, by-ref, variadic).
//
//   php -n -d extension=<revolt.so> tools/dump-api.php > ext.json      the extension
//   php -n tools/dump-api.php phplib > tools/api-1.0.9.json            the PHP library
//                                                                        (tools/phplib, composer install)
//   php tools/dump-api.php diff a.json b.json                           exit 1 on differences
//
// The one expected difference (DIVERGENCES.md): revolt-rs has Driver\NativeDriver instead of the
// library's four driver classes. `diff <library> <extension>` reports it without failing (and
// fails if it is not exactly that: a driver class still there, or NativeDriver missing).
//
// Not dumped (they cannot match an internal implementation): file names and lines, doc comments,
// isInternal()/isUserDefined(), attributes (#[\Override]), ReflectionParameter::isPromoted().
// Default values are compared through var_export(), so `null` and `null` constants agree.

if (($argv[1] ?? '') === 'diff') {
    $a = json_decode(file_get_contents($argv[2]), true);
    $b = json_decode(file_get_contents($argv[3]), true);
    $diff = [];
    $expected = [];
    $removed = ['StreamSelectDriver', 'EvDriver', 'EventDriver', 'UvDriver'];
    if (isset($a['classes']['Revolt\\EventLoop\\Driver\\StreamSelectDriver'])) {
        foreach ($removed as $r) {
            $name = "Revolt\\EventLoop\\Driver\\$r";
            if (isset($a['classes'][$name]) && !isset($b['classes'][$name])) {
                unset($a['classes'][$name]);
                $expected[] = "- /classes/$name (expected: no such class in revolt-rs)";
            }
        }
        $native = 'Revolt\\EventLoop\\Driver\\NativeDriver';
        if (isset($b['classes'][$native]) && !isset($a['classes'][$native])) {
            unset($b['classes'][$native]);
            $expected[] = "+ /classes/$native (expected: revolt-rs's one driver class)";
        }
    }
    $walk = static function ($x, $y, string $path) use (&$walk, &$diff): void {
        if (is_array($x) && is_array($y)) {
            foreach (array_unique(array_merge(array_keys($x), array_keys($y))) as $k) {
                if (!array_key_exists($k, $x)) {
                    $diff[] = "+ $path/$k: " . json_encode($y[$k]);
                } elseif (!array_key_exists($k, $y)) {
                    $diff[] = "- $path/$k: " . json_encode($x[$k]);
                } else {
                    $walk($x[$k], $y[$k], "$path/$k");
                }
            }
            if (array_is_list($x) && array_is_list($y) && $x !== $y && count($x) === count($y)) {
                // order of lists matters (parameters, interfaces)
                foreach ($x as $i => $v) {
                    if ($v !== $y[$i] && !is_array($v)) {
                        $diff[] = "~ $path/$i: " . json_encode($v) . ' => ' . json_encode($y[$i]);
                    }
                }
            }
            return;
        }
        if ($x !== $y) {
            $diff[] = "~ $path: " . json_encode($x) . ' => ' . json_encode($y);
        }
    };
    $walk($a, $b, '');
    foreach ($expected as $d) {
        echo $d, "\n";
    }
    foreach ($diff as $d) {
        echo $d, "\n";
    }
    echo count($diff) === 0 ? "API surfaces are identical" . ($expected === [] ? "" : " (but for the expected difference)") . "\n" : count($diff) . " difference(s)\n";
    exit(count($diff) === 0 ? 0 : 1);
}

if (($argv[1] ?? '') === 'phplib') {
    if (extension_loaded('revolt')) {
        fwrite(STDERR, "the revolt extension is loaded: run the PHP library dump with php -n\n");
        exit(1);
    }
    $vendor = __DIR__ . '/phplib/vendor';
    require $vendor . '/autoload.php';
    $src = $vendor . '/revolt/event-loop/src';
    $it = new RecursiveIteratorIterator(new RecursiveDirectoryIterator($src, FilesystemIterator::SKIP_DOTS));
    foreach ($it as $file) {
        $name = 'Revolt\\' . str_replace(['/', '.php'], ['\\', ''], substr($file->getPathname(), strlen($src) + 1));
        class_exists($name) || interface_exists($name) || enum_exists($name) || throw new Error("cannot load $name");
    }
} elseif (!extension_loaded('revolt') && !class_exists('Revolt\EventLoop', false)) {
    fwrite(STDERR, "neither the revolt extension nor the Revolt classes are loaded\n");
    exit(1);
}

$type = static fn (?ReflectionType $t): ?string => $t === null ? null : (string) $t;
$export = static fn (mixed $v): string => $v instanceof UnitEnum ? $v::class . '::' . $v->name : var_export($v, true);
$params = static function (ReflectionFunctionAbstract $f) use ($type, $export): array {
    $r = [];
    foreach ($f->getParameters() as $p) {
        $r[] = [
            'name' => $p->getName(),
            'type' => $type($p->getType()),
            'optional' => $p->isOptional(),
            'byref' => $p->isPassedByReference(),
            'variadic' => $p->isVariadic(),
            'default' => $p->isDefaultValueAvailable() ? $export($p->getDefaultValue()) : null,
        ];
    }
    return $r;
};

$names = array_merge(get_declared_classes(), get_declared_interfaces());
$names = array_filter($names, static fn (string $n): bool => str_starts_with($n, 'Revolt\\') && !str_contains($n, '@anonymous'));
sort($names);
$out = [];
foreach ($names as $name) {
    $c = new ReflectionClass($name);
    $methods = [];
    foreach ($c->getMethods() as $m) {
        if ($m->getDeclaringClass()->getName() !== $c->getName()) {
            continue;
        }
        $methods[$m->getName()] = [
            'modifiers' => implode(' ', Reflection::getModifierNames($m->getModifiers())),
            'params' => $params($m),
            'return' => $type($m->getReturnType()),
            'tentative' => $type($m->getTentativeReturnType()),
            'required' => $m->getNumberOfRequiredParameters(),
            'byref_return' => $m->returnsReference(),
            'constructor' => $m->isConstructor(),
        ];
    }
    ksort($methods);
    $constants = [];
    foreach ($c->getReflectionConstants() as $k) {
        if ($k->getDeclaringClass()->getName() !== $c->getName()) {
            continue;
        }
        $constants[$k->getName()] = [
            'value' => $export($k->getValue()),
            'modifiers' => implode(' ', Reflection::getModifierNames($k->getModifiers())),
            'case' => $k->isEnumCase(),
            'type' => method_exists($k, 'getType') ? $type($k->getType()) : null,
        ];
    }
    ksort($constants);
    $props = [];
    foreach ($c->getProperties() as $p) {
        if ($p->getDeclaringClass()->getName() !== $c->getName()) {
            continue;
        }
        $props[$p->getName()] = [
            'type' => $type($p->getType()),
            'modifiers' => implode(' ', Reflection::getModifierNames($p->getModifiers())),
            'readonly' => $p->isReadOnly(),
            'static' => $p->isStatic(),
            'has_default' => $p->hasDefaultValue(),
            'default' => $p->hasDefaultValue() ? $export($p->getDefaultValue()) : null,
        ];
    }
    ksort($props);
    $out[$name] = [
        'kind' => $c->isEnum() ? 'enum' : ($c->isInterface() ? 'interface' : ($c->isTrait() ? 'trait' : 'class')),
        'modifiers' => implode(' ', Reflection::getModifierNames($c->getModifiers())),
        'final' => $c->isFinal(),
        'abstract' => $c->isAbstract(),
        'readonly' => method_exists($c, 'isReadOnly') ? $c->isReadOnly() : false,
        'instantiable' => $c->isInstantiable(),
        'cloneable' => $c->isCloneable(),
        'parent' => $c->getParentClass() ? $c->getParentClass()->getName() : null,
        'interfaces' => $c->getInterfaceNames(),
        'backed' => $c->isEnum() ? (new ReflectionEnum($name))->isBacked() : null,
        'constants' => $constants,
        'properties' => $props,
        'methods' => $methods,
    ];
}
echo json_encode(['php' => PHP_MAJOR_VERSION . '.' . PHP_MINOR_VERSION, 'classes' => $out], JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_PRESERVE_ZERO_FRACTION), "\n";
