# Divergences from revolt/event-loop 1.0.9

revolt-rs implements the API and behaviour of revolt/event-loop 1.0.9; revolt/event-loop's own
test suite passes against it unchanged (except the tests listed in
[tests/NOT_APPLICABLE.md](tests/NOT_APPLICABLE.md)). This file lists every deliberate difference.
Each entry names the test of `tests/native` that pins it. Those tests skip in the baseline run
against the PHP library (`REVOLT_RS_PHPLIB=1 tools/test.sh`); every other test of `tests/native`
passes on both, so it shows behaviour that is the same.

## Reflection

- Classes, methods and properties are internal (`isInternal()`), without file names, lines, doc
  comments or attributes (`#[\Override]`); constructor parameters are not "promoted". Everything
  else Reflection shows (names, modifiers, types, defaults, constants, parents, interfaces) is
  identical: `tools/dump-api.php diff` against the PHP library, on every PHP version in CI.
  Pinned by `ReflectionTest::testClassesAreInternal`.
- The private properties of the PHP classes are declared (Reflection lists them) but the native
  classes keep their state in Rust: they stay at their defaults/uninitialised. Exceptions:
  `EventLoop::$driver` (the loop's driver, set when it is made), the DriverCallback properties, TimerQueue's
  arrays, FiberLocal's `$initializer` and InvalidCallbackError's properties, which are real.
  Pinned by `ReflectionTest::testPrivateStateStaysAtDefaults`.
- The private methods (`tick()`, `invokeCallbacks()`, `selectStreams()`, ...) exist but do
  nothing (`isEmpty()` and `callbackId()` work); they are reachable through Reflection only.
  Pinned by `ReflectionTest::testPrivateMethodsDoNothing`.

## Behaviour

- **One driver class.** revolt-rs has `Revolt\EventLoop\Driver\NativeDriver` (final, extends
  `Internal\AbstractDriver`, `getHandle()` is null) instead of revolt/event-loop's
  `StreamSelectDriver`, `EvDriver`, `EventDriver` and `UvDriver`, which do not exist.
  `EventLoop::getDriver()` and `DriverFactory::create()` return a NativeDriver (the factory wraps it
  in a `TracingDriver` with `REVOLT_DRIVER_DEBUG_TRACE`, as before); `REVOLT_DRIVER` selects nothing
  and is ignored (`DriverFactory::createDriverFromEnv()` returns null). The rest of the API is
  unchanged (`TracingDriver`, `DriverFactory`, PHP subclasses of `AbstractDriver`). Revolt's
  driver suite runs against NativeDriver (tests/NOT_APPLICABLE.md). Pinned by
  `NativeDriverTest::testOnlyNativeDriver` and `SetDriverTest`; `tools/dump-api.php diff` reports
  exactly this as the expected difference.
- **The loop's driver cannot be replaced.** `EventLoop::setDriver()` always throws
  `UnsupportedFeatureException("Revolt\EventLoop::setDriver() is not supported: the event loop
  always runs on its native driver")` (after its argument check) and changes nothing:
  `EventLoop::getDriver()` is always the native driver the loop made, so extensions embedding
  revolt-rs (a database driver's I/O, say) always share one native loop with PHP code. Writing
  `EventLoop::$driver` (Reflection, a bound closure) does not change it either. The loop makes its
  driver as a NativeDriver; `REVOLT_DRIVER_DEBUG_TRACE` is ignored there. Other NativeDriver
  objects, `TracingDriver` and PHP subclasses of `AbstractDriver` work when used directly
  (`$driver->run()`), they just never become the loop's. Revolt's own tests that swap the driver are excluded (tests/NOT_APPLICABLE.md). Pinned by `SetDriverTest` and
  `tests/native/destruction_order.phpt`.
- **No FD_SETSIZE limit.** NativeDriver uses epoll/kqueue/WSAPoll: any number of streams,
  no "You have reached the limits of stream_select()" exception. Pinned by
  `NoFdSetSizeLimitTest`.
- **Signals without ext-pcntl.** onSignal() works on Unix (a sigaction handler of the
  extension); unlike revolt/event-loop's StreamSelectDriver, NativeDriver does not need pcntl. When the last callback of a
  signal goes, the disposition the signal had before is restored (revolt/event-loop sets
  SIG_DFL). Registration failures throw `UnsupportedFeatureException("Failed to register signal
  handler; Errno: N; <strerror>")` from the loop, as Revolt's do (the message text differs).
  Windows: `UnsupportedFeatureException("Signal handling requires the pcntl extension")`. Pinned
  by `SignalTest`.
- **Data in PHP's stream read buffer** makes a readable callback fire at once, as with
  revolt/event-loop's StreamSelectDriver (its ev/event/uv drivers do not look at the buffer).
  Pinned by `StreamTest::testBufferedDataIsReadable` (which the library's StreamSelectDriver
  passes too).
- **Streams that are not streams.** A readable/writable callback whose stream is not a valid
  stream resource when it is activated (closed, or not a stream) throws a `TypeError` from the
  loop ("Invalid stream for callback x: ..."); revolt/event-loop fails in stream_select() or an
  assertion. Pinned by `StreamTest::testClosedStreamIsTypeError`.
- **Extra arguments** to the native methods are ignored, as PHP functions ignore them; missing or
  wrongly typed ones throw PHP's internal-function errors (`ArgumentCountError: ... expects
  exactly 2 arguments, 1 given`, `TypeError` with the same wording as for typed PHP functions)
  instead of the messages of user functions ("Too few arguments to function ..."). Pinned by
  `ObjectTest::testInternalArgumentErrors`.
- **Cloning** a driver, a suspension or a TracingDriver throws `Error("Trying to clone an
  uncloneable object of class ...")` (their native state cannot be shared safely); Reflection
  still reports them cloneable, as the PHP classes are. Pinned by `ObjectTest::testCloneIsRefused`.
- **`new DriverSuspension(...)`** throws `Error`: suspensions come from `getSuspension()` only.
  Pinned by `ObjectTest::testSuspensionsComeFromTheDriver`.
- **A suspension whose fiber was destroyed** (collected while suspended) ignores resume() and
  throw(). revolt/event-loop resumes `{main}`'s loop with the value instead once its weak
  reference to the fiber comes back null (after the fiber is freed); while the fiber is being
  destroyed (a `finally` block of the fiber resuming its own suspension) both ignore it, which
  `SuspensionTest::testResumeOfDestroyedFiberIsIgnored` checks on both.
- **`{main}` suspend() while the loop is executing** (from a userland driver's dispatch()
  hook, say) throws `Error("The event loop is already running")`; Revolt's assertion/fiber
  checks fail differently. Pinned by `SuspensionTest::testMainSuspendDuringDispatch`.
- **Interrupt descriptions.** "Interrupt from event loop must throw an exception: ..." and
  UncaughtThrowable messages for exceptions thrown out of fibers resumed by a suspension name
  `Revolt\EventLoop\Internal\DriverSuspension::resume` (or `::throw`) instead of the closure of
  DriverSuspension.php with its file and line. Pinned by
  `SuspensionTest::testUncaughtFromResumedFiberNamesResume`.
- **TracingDriver's wrapper closures** are named `Revolt\EventLoop\Driver\TracingDriver::{closure}`
  in error messages (PHP 8.4+ names Revolt's `{closure:...::defer():NN}`, with a file and line).
  Pinned by `TracingDriverDumpTest::testWrapperClosureName`.
- **A PHP subclass of AbstractDriver** that writes to a DriverCallback's public properties
  (`$callback->enabled = false`) changes only that object: the loop keeps its own state, written
  through to the objects it hands out. A subclass that does not call `parent::__construct()`
  works (revolt/event-loop fails on its uninitialised properties). Pinned by
  `UserlandDriverTest::testCallbackPropertyWritesAreLocal` and `::testWithoutParentConstructor`.
- **Equal timer expirations** fire in creation order (Revolt: heap order, unspecified). Pinned by
  `TimerOrderTest`.
