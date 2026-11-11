<?php

declare(strict_types=1);

namespace RevoltRs\Tests;

/**
 * Tests of deliberate divergences (DIVERGENCES.md) skip in the baseline run against the PHP
 * library (REVOLT_RS_PHPLIB=1); every other test of tests/native passes on both.
 */
trait NativeOnly
{
    private static function nativeOnly(): void
    {
        if (\getenv('REVOLT_RS_PHPLIB')) {
            self::markTestSkipped('a divergence of revolt-rs from revolt/event-loop (DIVERGENCES.md)');
        }
    }
}
