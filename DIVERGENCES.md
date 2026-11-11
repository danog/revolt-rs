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
  `EventLoop::$driver` (written by setDriver()), the DriverCallback properties, TimerQueue's
  arrays, FiberLocal's `$initializer` and InvalidCallbackError's properties, which are real.
  Pinned by `ReflectionTest::testPrivateStateStaysAtDefaults`.
- The private methods (`tick()`, `invokeCallbacks()`, `selectStreams()`, ...) exist but do
  nothing (`isEmpty()` and `callbackId()` work); they are reachable through Reflection only.
  Pinned by `ReflectionTest::testPrivateMethodsDoNothing`.
- `EventLoop::setDriver()`'s temporary driver is an instance of the internal class
  `Revolt\EventLoop\Internal\AbstractDriver@anonymous` (PHP's name for it ends with the
  file and line). Pinned by `SetDriverTest::testTemporaryDriverClass`.

## Behaviour

- **No FD_SETSIZE limit.** StreamSelectDriver uses epoll/kqueue/WSAPoll: any number of streams,
  no "You have reached the limits of stream_select()" exception. Pinned by
  `NoFdSetSizeLimitTest`.
- **All four driver classes share the native reactor.** EvDriver, EventDriver and UvDriver keep
  `isSupported()` (ext-ev/event/uv loaded), their constructor's requirement of the extension and
  `getHandle()` (an `\EvLoop`, `\EventBase` or uv loop created at construction) — but that
  handle is not driven by the loop: watchers user code adds to it directly never fire. Pinned by
  `DriverHandleTest`.
- **Signals without ext-pcntl.** onSignal() works on Unix for every driver class (a sigaction
  handler of the extension); StreamSelectDriver does not need pcntl. When the last callback of a
  signal goes, the disposition the signal had before is restored (revolt/event-loop sets
  SIG_DFL). Registration failures throw `UnsupportedFeatureException("Failed to register signal
  handler; Errno: N; <strerror>")` from the loop, as Revolt's do (the message text differs).
  Windows: `UnsupportedFeatureException("Signal handling requires the pcntl extension")`. Pinned
  by `SignalTest`.
- **Data in PHP's stream read buffer** makes a readable callback fire at once for every driver
  class (stream_select does this; ev/event/uv do not). Pinned by
  `StreamTest::testBufferedDataIsReadable`.
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
  For the same reason (`EventLoop` is an internal class), the creation traces of
  `TracingDriver::dump()` have no `EventLoop.php:NN` frame for calls made through the facade.
  Pinned by `TracingDriverDumpTest::testWrapperClosureName` and
  `SetDriverTest::testDumpThroughFacadeHasNoEventLoopFrame`.
- **A PHP subclass of AbstractDriver** that writes to a DriverCallback's public properties
  (`$callback->enabled = false`) changes only that object: the loop keeps its own state, written
  through to the objects it hands out. A subclass that does not call `parent::__construct()`
  works (revolt/event-loop fails on its uninitialised properties). Pinned by
  `UserlandDriverTest::testCallbackPropertyWritesAreLocal` and `::testWithoutParentConstructor`.
- **Equal timer expirations** fire in creation order (Revolt: heap order, unspecified). Pinned by
  `TimerOrderTest`.
