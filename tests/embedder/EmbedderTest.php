<?php

declare(strict_types=1);

namespace RevoltRs\Tests\Embedder;

use PHPUnit\Framework\TestCase;

/**
 * examples/embedder (revolt_embed_example) loaded alone, with the standalone extension in both
 * orders, with incompatible owners and with the PHP library. Each case runs scenario.php in a php process of its own (php -n plus the extensions
 * named by the environment; tools/embed-test.sh builds them and sets it):
 *
 * - REVOLT_EMBED_EXT: the embedder extension (required);
 * - REVOLT_EXT: the standalone revolt extension;
 * - REVOLT_EXT_ABI_MAJOR / REVOLT_EXT_ABI_SIZE: revolt built with the test-abi-major-mismatch /
 *   test-abi-size-mismatch feature (its ABI table must be refused).
 */
final class EmbedderTest extends TestCase
{
    private static function ext(string $name): string
    {
        $path = (string) \getenv($name);
        if ($path === '') {
            self::markTestSkipped("$name is not set (tools/embed-test.sh)");
        }
        return $path;
    }

    /** @param list<string> $extensions */
    private static function runScenario(array $extensions, string $scenario, array $env = []): array
    {
        $cmd = [\PHP_BINARY, '-n', '-d', 'zend.assertions=1'];
        foreach ($extensions as $extension) {
            $cmd[] = '-d';
            $cmd[] = 'extension=' . $extension;
        }
        $cmd[] = __DIR__ . '/scenario.php';
        $cmd[] = $scenario;
        $env = \array_merge(\getenv(), ['REVOLT_EMBED_EXAMPLE_NO_REGISTER' => '0', 'REVOLT_EMBED_SCENARIO_PHPLIB' => ''], $env);
        $asan = (string) \getenv('REVOLT_ASAN_RT');
        if ($asan !== '') {
            // tools/asan.sh embed: macOS' dyld does not hand DYLD_* down, so preload it again
            $env[\PHP_OS_FAMILY === 'Darwin' ? 'DYLD_INSERT_LIBRARIES' : 'LD_PRELOAD'] = $asan;
        }
        $proc = \proc_open($cmd, [1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes, null, $env);
        $out = \stream_get_contents($pipes[1]);
        $err = \stream_get_contents($pipes[2]);
        $rc = \proc_close($proc);
        self::assertSame(0, $rc, "exit code $rc\n$out\n$err");
        self::assertSame('', $err);
        $result = \json_decode($out, true);
        self::assertIsArray($result, $out);
        return $result;
    }

    /** What every case does the same, natively or through PHP. */
    private static function assertWorks(array $r, string $path, bool $streams = true, bool $phpLibrary = false): void
    {
        self::assertSame($path, $r['sleep_main']);
        self::assertTrue($r['slept_enough']);
        self::assertSame([[1, $path], [2, $path], [0, $path]], $r['sleep_fibers']);
        self::assertSame(["embedder $path", 'php timer'], $r['interleaved']);
        if ($streams) {
            self::assertSame($path, $r['wait_readable']);
            self::assertSame('x', $r['read']);
        }
        self::assertSame($path, $r['src_wait']);
        self::assertSame('x', $r['src_read']);
        self::assertSame($path === 'native' ? 'ready' : $path, $r['src_ready']);
        self::assertSame('y', $r['src_read_ready']);
        self::assertSame([$path, $path, $path], $r['src_fibers']);
        self::assertSame($path === 'native' ? ['takes' => 3, 'flushes' => 1] : ['takes' => 0, 'flushes' => 0], $r['src_batch']);
        self::assertContains($r['src_writable'], $path === 'native' ? ['native', 'ready'] : ['php']);
        self::assertSame($path, $r['src_reused']);
        self::assertSame('from a callback', $r['uncaught']);
        self::assertSame([], $r['identifiers_after_uncaught']);
        if ($path === 'native') {
            self::assertSame(['type' => 'Delay', 'listed' => true, 'referenced' => true], $r['native_id']);
            // 3 + 1 sleeps, 1 wait, the cancelled delay; then the sleep the uncaught exception ended
            self::assertSame(7, $r['released']);
            self::assertSame(8, $r['released_after_uncaught']);
        } else {
            self::assertNull($r['native_id']);
            self::assertSame(0, $r['released_after_uncaught']);
        }
        if ($path === 'native' || !$phpLibrary) {
            self::assertSame('Revolt\\EventLoop::setDriver() is not supported: the event loop always runs on its native driver', $r['set_driver']);
            self::assertTrue($r['same_driver']);
        }
    }

    public function testEmbedderAlone(): void
    {
        $r = self::runScenario([self::ext('REVOLT_EMBED_EXT')], 'default');
        self::assertSame('native', $r['mode']);
        self::assertTrue($r['owner']);
        self::assertTrue($r['internal_classes']);
        self::assertSame('revolt_embed_example', $r['classes_module']);
        self::assertWorks($r, 'native');
    }

    public function testStandaloneFirst(): void
    {
        $r = self::runScenario([self::ext('REVOLT_EXT'), self::ext('REVOLT_EMBED_EXT')], 'default');
        self::assertSame('native-foreign', $r['mode']);
        self::assertFalse($r['owner']);
        self::assertSame('revolt', $r['classes_module']);
        self::assertWorks($r, 'native');
    }

    public function testEmbedderFirst(): void
    {
        $r = self::runScenario([self::ext('REVOLT_EMBED_EXT'), self::ext('REVOLT_EXT')], 'default');
        self::assertSame('native', $r['mode']);
        self::assertTrue($r['owner']);
        self::assertSame('revolt_embed_example', $r['classes_module']);
        self::assertWorks($r, 'native');
    }

    public static function incompatible(): iterable
    {
        yield 'other ABI major' => ['REVOLT_EXT_ABI_MAJOR'];
        yield 'smaller ABI table' => ['REVOLT_EXT_ABI_SIZE'];
    }

    /** @dataProvider incompatible */
    public function testIncompatibleOwnerFallsBackToPhp(string $env): void
    {
        $r = self::runScenario([self::ext($env), self::ext('REVOLT_EMBED_EXT')], 'default');
        self::assertSame('php', $r['mode']);
        self::assertFalse($r['owner']);
        self::assertSame('revolt', $r['classes_module']);
        self::assertWorks($r, 'php');
    }

    public function testPhpLibraryFallback(): void
    {
        $r = self::runScenario([self::ext('REVOLT_EMBED_EXT')], 'default', [
            'REVOLT_EMBED_EXAMPLE_NO_REGISTER' => '1',
            'REVOLT_EMBED_SCENARIO_PHPLIB' => '1',
        ]);
        self::assertSame('php', $r['mode']);
        self::assertFalse($r['owner']);
        self::assertFalse($r['internal_classes']);
        self::assertNull($r['classes_module']);
        self::assertWorks($r, 'php', true, true);
    }

    /** Native callbacks and waits pending at the end of the script are freed at shutdown. */
    public function testPendingAtShutdown(): void
    {
        foreach ([[self::ext('REVOLT_EMBED_EXT')], [self::ext('REVOLT_EXT'), self::ext('REVOLT_EMBED_EXT')]] as $extensions) {
            self::assertSame(['pending' => true], self::runScenario($extensions, 'pending'));
        }
    }
}
