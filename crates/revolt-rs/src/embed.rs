//! The embedding API: drive the loop of the calling thread from Rust, without PHP-level calls,
//! whichever copy of revolt-rs registered the classes (see EMBEDDING.md).
//!
//! - The copy that registered the classes ("owner") publishes [`Abi`], a versioned C-ABI table.
//!   Every copy exports one as the symbol `revolt_rs_abi_v1`; a copy that is not the owner finds
//!   the owner's from the address of the handler of `Revolt\EventLoop::getDriver` (dladdr +
//!   dlsym on Unix, GetModuleHandleEx + GetProcAddress on Windows).
//! - [`NativeLoop::current`] gives the thread's driver (always a native one: `setDriver()` is
//!   refused) through the owner's table when that is compatible. Otherwise (an incompatible
//!   owner, or classes that are not revolt-rs's) the embedder may call `Revolt\EventLoop` through
//!   PHP ([`php`]).
//! - Sources: raw sockets registered edge-triggered in the driver's poller, delivered to the
//!   embedder in batches (`src_*`).
//! - Callbacks are Rust functions `cb(data, id)`; `release(data)` (optional) runs exactly once,
//!   when the callback is gone (fired once, cancelled, or its driver destroyed). Neither may
//!   call back into the loop for the same callback from `release`.
//! - Ids are real Revolt callbacks of the driver (visible to getIdentifiers(), cancellable by
//!   their string id); the API uses their numbers ("a" = 1).

use crate::classes::{self, suspension};
use crate::core::run::{self, D};
pub use crate::core::backend::{SrcFd, SrcFlushFn, SrcTakeFn, READ as SRC_READ, WRITE as SRC_WRITE};
use crate::core::{Cb, Microtask, NativeFn, ReleaseFn};
use crate::zend::{call, stream::Handle, sys, zval, zval::Zv};
use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

/// The ABI major version: a copy uses another copy's table only with the same major and a size
/// at least as large as its own.
pub const ABI_MAJOR: u32 = 1;
/// 1: the sources (`src_*`); 2: `idle`
pub const ABI_MINOR: u32 = 2;

/// The C-ABI function table of a revolt-rs copy. Drivers and suspensions are `zend_object`s;
/// functions returning bool return false with a PHP exception pending (or for an invalid id).
#[repr(C)]
pub struct Abi {
    pub size: usize,
    pub abi_major: u32,
    pub abi_minor: u32,
    /// revolt-rs version, NUL-terminated
    pub version: *const c_char,
    /// revolt/event-loop version implemented, NUL-terminated
    pub revolt_version: *const c_char,
    /// The thread's driver (EventLoop::getDriver(), created if needed; always native since
    /// minor 1, which refuses setDriver()); null with an exception pending. Borrowed.
    pub current: unsafe extern "C" fn() -> *mut sys::zend_object,
    /// onReadable/onWritable on an OS handle (an fd; a SOCKET on Windows when `socket`). The
    /// callback functions take `(f, data, release, held)`: see [`NativeLoop::delay`].
    pub on_handle: unsafe extern "C" fn(*mut sys::zend_object, i64, bool, bool, NativeFn, *mut c_void, Option<ReleaseFn>, *mut sys::zend_object) -> u64,
    /// delay ($repeat false) / repeat; 0 with an exception when the interval is negative
    pub timer: unsafe extern "C" fn(*mut sys::zend_object, f64, bool, NativeFn, *mut c_void, Option<ReleaseFn>, *mut sys::zend_object) -> u64,
    pub defer: unsafe extern "C" fn(*mut sys::zend_object, NativeFn, *mut c_void, Option<ReleaseFn>, *mut sys::zend_object) -> u64,
    /// a microtask: `f(data, 0)` runs before any other callback (no release: run exactly once)
    pub queue: unsafe extern "C" fn(*mut sys::zend_object, NativeFn, *mut c_void),
    pub cancel: unsafe extern "C" fn(*mut sys::zend_object, u64),
    pub enable: unsafe extern "C" fn(*mut sys::zend_object, u64) -> bool,
    pub disable: unsafe extern "C" fn(*mut sys::zend_object, u64),
    pub reference: unsafe extern "C" fn(*mut sys::zend_object, u64) -> bool,
    pub unreference: unsafe extern "C" fn(*mut sys::zend_object, u64),
    /// getSuspension() of the current fiber or {main}: a new reference
    pub suspension: unsafe extern "C" fn(*mut sys::zend_object) -> *mut sys::zend_object,
    /// $suspension->suspend(): writes the value to `rv` (owned); false with an exception
    pub suspend: unsafe extern "C" fn(*mut sys::zend_object, *mut sys::zval) -> bool,
    /// $suspension->resume($value) (the value is copied); false with an exception
    pub resume: unsafe extern "C" fn(*mut sys::zend_object, *mut sys::zval) -> bool,
    /// $suspension->throw($exception) (a new reference is taken); false with an exception
    pub throw: unsafe extern "C" fn(*mut sys::zend_object, *mut sys::zend_object) -> bool,
    /// Keeps an object alive (a new reference) for the running fiber or {main}, visible to the
    /// cycle collector as a local variable of the fiber would be, until the matching `unhold`
    /// (LIFO, on the same fiber).
    pub hold: unsafe extern "C" fn(*mut sys::zend_object),
    /// Releases the last `hold` of the running fiber or {main}.
    pub unhold: unsafe extern "C" fn(),
    // ---- minor 1: sources (see EMBEDDING.md, "Sources")
    /// A new source group on the driver: its id (0: none, with an exception pending).
    pub src_group: unsafe extern "C" fn(*mut sys::zend_object, SrcTakeFn, SrcFlushFn, *mut c_void) -> u32,
    /// Closes a group (its sources are forgotten; unreferenced).
    pub src_close: unsafe extern "C" fn(*mut sys::zend_object, u32),
    /// References / unreferences a group: while referenced, the loop runs (as for a referenced
    /// callback).
    pub src_reference: unsafe extern "C" fn(*mut sys::zend_object, u32, bool),
    /// Arms `n` sources of a group for the wait `token`: 1 when one was already ready (consumed,
    /// nothing armed), 0 when armed, -errno when the poller refused one.
    pub src_arm: unsafe extern "C" fn(*mut sys::zend_object, u32, *const SrcFd, usize, u64) -> i32,
    /// The wait `token` over `n` sources ended.
    pub src_disarm: unsafe extern "C" fn(*mut sys::zend_object, *const SrcFd, usize, u64),
    /// A source is about to be closed.
    pub src_forget: unsafe extern "C" fn(*mut sys::zend_object, i64),
    // ---- minor 2
    /// Whether the loop has nothing to do (O(1)): no enabled callback (referenced or not:
    /// watchers, timers, defers, signals) except `except` (an id of the caller's, 0: none), no
    /// queued callback or microtask, no armed source, no pending interrupt, and the loop not
    /// running. Then a `{main}` call may block in its own I/O instead of running the loop: nothing
    /// but that call's own I/O could happen meanwhile (only this thread runs PHP code).
    pub idle: unsafe extern "C" fn(*mut sys::zend_object, u64) -> bool,
}

unsafe impl Sync for Abi {}

unsafe extern "C" fn abi_current() -> *mut sys::zend_object {
    // always a NativeDriver (setDriver() is refused)
    classes::eventloop::driver()
}

unsafe fn native_cb(f: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> Cb {
    let held = if held.is_null() { Zv::undef() } else { Zv(zval::obj_ref(held)) };
    Cb::Native { f, data, release, held }
}

unsafe extern "C" fn abi_on_handle(d: *mut sys::zend_object, fd: i64, socket: bool, writable: bool, f: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
    run::stream_handle(D(d), Handle { fd, socket }, native_cb(f, data, release, held), writable)
}

unsafe extern "C" fn abi_timer(d: *mut sys::zend_object, secs: f64, repeat: bool, f: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
    if secs < 0.0 {
        drop(native_cb(f, data, release, held));
        call::throw_error(if repeat { "Interval must be greater than or equal to zero" } else { "Delay must be greater than or equal to zero" });
        return 0;
    }
    run::timer(D(d), secs, native_cb(f, data, release, held), repeat).unwrap_or(0)
}

unsafe extern "C" fn abi_defer(d: *mut sys::zend_object, f: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
    run::defer(D(d), native_cb(f, data, release, held))
}

unsafe extern "C" fn abi_queue(d: *mut sys::zend_object, f: NativeFn, data: *mut c_void) {
    D(d).c().microtasks.push_back(Microtask::Native { f, data });
}

unsafe extern "C" fn abi_cancel(d: *mut sys::zend_object, id: u64) {
    run::v_cancel(D(d), id);
}

unsafe extern "C" fn abi_enable(d: *mut sys::zend_object, id: u64) -> bool {
    run::enable(D(d), id) == Some(true)
}

unsafe extern "C" fn abi_disable(d: *mut sys::zend_object, id: u64) {
    run::v_disable(D(d), id);
}

unsafe extern "C" fn abi_reference(d: *mut sys::zend_object, id: u64) -> bool {
    run::reference(D(d), id)
}

unsafe extern "C" fn abi_unreference(d: *mut sys::zend_object, id: u64) {
    run::unreference(D(d), id)
}

unsafe extern "C" fn abi_suspension(d: *mut sys::zend_object) -> *mut sys::zend_object {
    suspension::get(D(d))
}

unsafe extern "C" fn abi_suspend(s: *mut sys::zend_object, rv: *mut sys::zval) -> bool {
    let mut r = suspension::suspend_value(s);
    if call::has_exception() {
        return false;
    }
    zval::write(rv, r.take());
    true
}

unsafe extern "C" fn abi_resume(s: *mut sys::zend_object, v: *mut sys::zval) -> bool {
    let value = if v.is_null() { Zv::null() } else { Zv(zval::copy_deref(v)) };
    suspension::wake(s, value, false);
    !call::has_exception()
}

unsafe extern "C" fn abi_throw(s: *mut sys::zend_object, e: *mut sys::zend_object) -> bool {
    suspension::wake(s, Zv(zval::obj_ref(e)), true);
    !call::has_exception()
}

unsafe extern "C" fn abi_src_group(d: *mut sys::zend_object, take: SrcTakeFn, flush: SrcFlushFn, data: *mut c_void) -> u32 {
    D(d).c().reactor.src_group(take, flush, data)
}

unsafe extern "C" fn abi_src_close(d: *mut sys::zend_object, g: u32) {
    let c = D(d).c();
    if c.reactor.src_close(g) {
        c.live -= 1;
    }
}

unsafe extern "C" fn abi_src_reference(d: *mut sys::zend_object, g: u32, on: bool) {
    let c = D(d).c();
    if c.reactor.src_set_referenced(g, on) == Some(true) {
        if on {
            c.live += 1;
        } else {
            c.live -= 1;
        }
    }
}

unsafe extern "C" fn abi_src_arm(d: *mut sys::zend_object, g: u32, fds: *const SrcFd, n: usize, token: u64) -> i32 {
    let fds = if n == 0 { &[][..] } else { std::slice::from_raw_parts(fds, n) };
    match D(d).c().reactor.src_arm(g, fds, token) {
        Ok(hit) => hit as i32,
        Err(e) => -e.max(1),
    }
}

unsafe extern "C" fn abi_src_disarm(d: *mut sys::zend_object, fds: *const SrcFd, n: usize, token: u64) {
    let fds = if n == 0 { &[][..] } else { std::slice::from_raw_parts(fds, n) };
    D(d).c().reactor.src_disarm(fds, token)
}

unsafe extern "C" fn abi_src_forget(d: *mut sys::zend_object, fd: i64) {
    D(d).c().reactor.src_forget(fd)
}

unsafe extern "C" fn abi_idle(d: *mut sys::zend_object, except: u64) -> bool {
    let c = D(d).c();
    let mut n = c.enabled_n;
    if n == 1 && except != 0 && c.callbacks.get_ref(except).is_some_and(|r| r.enabled) {
        n = 0;
    }
    n == 0
        && c.callback_queue.is_empty()
        && c.microtasks.is_empty()
        && c.enable_queue_len == 0
        && c.defer_queue_len == 0
        && c.interrupt.is_none()
        && c.in_loop.is_null()
        && c.reactor.src_armed() == 0
}

// test builds only (tools/embed-test.sh): an owner whose table another copy must refuse
#[cfg(feature = "test-abi-major-mismatch")]
const EXPORTED_MAJOR: u32 = ABI_MAJOR + 1;
#[cfg(not(feature = "test-abi-major-mismatch"))]
const EXPORTED_MAJOR: u32 = ABI_MAJOR;
#[cfg(feature = "test-abi-size-mismatch")]
const EXPORTED_SIZE: usize = std::mem::size_of::<Abi>() - std::mem::size_of::<usize>();
#[cfg(not(feature = "test-abi-size-mismatch"))]
const EXPORTED_SIZE: usize = std::mem::size_of::<Abi>();

unsafe extern "C" fn abi_hold(o: *mut sys::zend_object) {
    let f = crate::zend::active_fiber();
    if !f.is_null() {
        crate::zend::fiber::inflight::track(f);
    }
    crate::zend::fiber::inflight::hold(f, [zval::obj_ref(o)]);
}

unsafe extern "C" fn abi_unhold() {
    for z in crate::zend::fiber::inflight::take(crate::zend::active_fiber()).iter_mut() {
        zval::dtor(z);
    }
}

/// This copy's table (exported for the other copies).
#[no_mangle]
#[used]
pub static revolt_rs_abi_v1: Abi = Abi {
    size: EXPORTED_SIZE,
    abi_major: EXPORTED_MAJOR,
    abi_minor: ABI_MINOR,
    version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char,
    revolt_version: c"1.0.9".as_ptr(),
    current: abi_current,
    on_handle: abi_on_handle,
    timer: abi_timer,
    defer: abi_defer,
    queue: abi_queue,
    cancel: abi_cancel,
    enable: abi_enable,
    disable: abi_disable,
    reference: abi_reference,
    unreference: abi_unreference,
    suspension: abi_suspension,
    suspend: abi_suspend,
    resume: abi_resume,
    throw: abi_throw,
    hold: abi_hold,
    unhold: abi_unhold,
    src_group: abi_src_group,
    src_close: abi_src_close,
    src_reference: abi_src_reference,
    src_arm: abi_src_arm,
    src_disarm: abi_src_disarm,
    src_forget: abi_src_forget,
    idle: abi_idle,
};

struct AbiRef(Option<&'static Abi>);
unsafe impl Send for AbiRef {}
unsafe impl Sync for AbiRef {}

static ACTIVE: OnceLock<AbiRef> = OnceLock::new();

/// MINIT of the owner: its own table is the active one.
pub fn publish() {
    let _ = ACTIVE.set(AbiRef(Some(&revolt_rs_abi_v1)));
}

/// The table of the copy that owns the Revolt classes, when compatible (None: the classes are
/// someone else's, e.g. the PHP library, or an incompatible revolt-rs). Resolved once (the
/// classes are registered at MINIT, before any call).
pub fn active() -> Option<&'static Abi> {
    ACTIVE.get_or_init(|| AbiRef(unsafe { find_owner() })).0
}

/// Whether the active table is another copy's (diagnostics).
pub fn is_foreign() -> bool {
    active().is_some_and(|a| !std::ptr::eq(a, &revolt_rs_abi_v1))
}

unsafe fn find_owner() -> Option<&'static Abi> {
    if crate::module::is_owner() {
        return Some(&revolt_rs_abi_v1);
    }
    let ce = call::lookup_class(b"revolt\\eventloop");
    if ce.is_null() || (*ce).type_ as u32 != 1 {
        return None;
    }
    let f = call::find_method(ce, b"getdriver");
    if f.is_null() || (*f).type_ as u32 != sys::ZEND_INTERNAL_FUNCTION {
        return None;
    }
    let addr = (*f).internal_function.handler.map(|h| h as *const c_void)?;
    let table = symbol_near(addr)?;
    let abi = &*(table as *const Abi);
    if abi.abi_major != ABI_MAJOR || abi.size < std::mem::size_of::<Abi>() {
        return None;
    }
    Some(abi)
}

/// `revolt_rs_abi_v1` of the shared object containing `addr`.
#[cfg(unix)]
unsafe fn symbol_near(addr: *const c_void) -> Option<*const c_void> {
    let mut info: libc::Dl_info = std::mem::zeroed();
    if libc::dladdr(addr, &mut info) == 0 || info.dli_fname.is_null() {
        return None;
    }
    let h = libc::dlopen(info.dli_fname, libc::RTLD_LAZY | libc::RTLD_NOLOAD);
    if h.is_null() {
        return None;
    }
    let s = libc::dlsym(h, c"revolt_rs_abi_v1".as_ptr());
    libc::dlclose(h);
    if s.is_null() {
        None
    } else {
        Some(s as *const c_void)
    }
}

#[cfg(windows)]
unsafe fn symbol_near(addr: *const c_void) -> Option<*const c_void> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleExW, GetProcAddress, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT};
    let mut m = std::ptr::null_mut();
    if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, addr as *const u16, &mut m) == 0 {
        return None;
    }
    GetProcAddress(m, c"revolt_rs_abi_v1".as_ptr() as *const u8).map(|p| p as *const c_void)
}

/// The fast path: the thread's native driver through the owner's table.
#[derive(Clone, Copy)]
pub struct NativeLoop {
    pub abi: &'static Abi,
    /// the driver object (borrowed: valid until the driver changes; fetch again per operation)
    pub driver: *mut sys::zend_object,
}

impl NativeLoop {
    /// The fast path for the thread's current driver, or None (use [`php`]). May create the
    /// default driver; None with an exception pending if that failed.
    pub fn current() -> Option<NativeLoop> {
        let abi = active()?;
        let d = unsafe { (abi.current)() };
        if d.is_null() {
            None
        } else {
            Some(NativeLoop { abi, driver: d })
        }
    }

    /// The driver's identity (compare to detect a setDriver()).
    pub fn driver(&self) -> *mut sys::zend_object {
        self.driver
    }

    /// onReadable(): `cb(data, id)` while the OS handle (an fd; a SOCKET on Windows when
    /// `socket`) is readable. See [`NativeLoop::delay`] for the other arguments.
    pub unsafe fn on_readable(&self, fd: i64, socket: bool, cb: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
        (self.abi.on_handle)(self.driver, fd, socket, false, cb, data, release, held)
    }

    pub unsafe fn on_writable(&self, fd: i64, socket: bool, cb: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
        (self.abi.on_handle)(self.driver, fd, socket, true, cb, data, release, held)
    }

    /// delay(): `cb(data, id)` once after `secs`. `release(data)` (optional) runs exactly once
    /// when the callback is gone (after it fired once, cancelled, or with its driver). `held`
    /// (nullable) is an object the callback keeps alive — a new reference is taken, released
    /// after `release` — and reports to PHP's cycle collector: pass the suspension the callback
    /// resumes there (not a reference owned by `data`), so that a fiber waiting on a driver
    /// that became garbage is collected as with PHP closures. Returns the id (0 with an
    /// exception pending).
    pub unsafe fn delay(&self, secs: f64, cb: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
        (self.abi.timer)(self.driver, secs, false, cb, data, release, held)
    }

    pub unsafe fn repeat(&self, secs: f64, cb: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
        (self.abi.timer)(self.driver, secs, true, cb, data, release, held)
    }

    pub unsafe fn defer(&self, cb: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: *mut sys::zend_object) -> u64 {
        (self.abi.defer)(self.driver, cb, data, release, held)
    }

    pub unsafe fn queue(&self, f: NativeFn, data: *mut c_void) {
        (self.abi.queue)(self.driver, f, data)
    }

    pub unsafe fn cancel(&self, id: u64) {
        (self.abi.cancel)(self.driver, id)
    }

    pub unsafe fn enable(&self, id: u64) -> bool {
        (self.abi.enable)(self.driver, id)
    }

    pub unsafe fn disable(&self, id: u64) {
        (self.abi.disable)(self.driver, id)
    }

    pub unsafe fn reference(&self, id: u64) -> bool {
        (self.abi.reference)(self.driver, id)
    }

    pub unsafe fn unreference(&self, id: u64) {
        (self.abi.unreference)(self.driver, id)
    }

    /// getSuspension(): a new reference to the suspension of the current fiber or {main}.
    pub unsafe fn suspension(&self) -> *mut sys::zend_object {
        (self.abi.suspension)(self.driver)
    }

    /// $suspension->suspend(): the value, or None with an exception pending.
    pub unsafe fn suspend(&self, s: *mut sys::zend_object) -> Option<Zv> {
        let mut rv = Zv::undef();
        if (self.abi.suspend)(s, rv.as_ptr()) {
            Some(rv)
        } else {
            None
        }
    }

    pub unsafe fn resume(&self, s: *mut sys::zend_object, value: *mut sys::zval) -> bool {
        (self.abi.resume)(s, value)
    }

    pub unsafe fn throw(&self, s: *mut sys::zend_object, e: *mut sys::zend_object) -> bool {
        (self.abi.throw)(s, e)
    }

    /// Keeps `o` alive for the running fiber (or {main}) where PHP's cycle collector sees it,
    /// until the matching [`NativeLoop::unhold`] on the same fiber. A reference a native frame
    /// owns while its fiber is suspended is invisible to the collector: a fiber waiting on a
    /// driver that became garbage could never be collected. Hold the suspension instead of
    /// owning it (see EMBEDDING.md, "Waiting").
    pub unsafe fn hold(&self, o: *mut sys::zend_object) {
        (self.abi.hold)(o)
    }

    pub unsafe fn unhold(&self) {
        (self.abi.unhold)()
    }

    /// A new source group on this driver (see EMBEDDING.md, "Sources"): `take(data, token)`
    /// is called during the loop's poll for each event of an armed source, `flush(data)` once
    /// after that poll when some `take` returned true. Its id; 0 with an exception pending.
    /// The group lives as long as the driver, or until [`NativeLoop::src_close`].
    pub unsafe fn src_group(&self, take: SrcTakeFn, flush: SrcFlushFn, data: *mut c_void) -> u32 {
        (self.abi.src_group)(self.driver, take, flush, data)
    }

    pub unsafe fn src_close(&self, g: u32) {
        (self.abi.src_close)(self.driver, g)
    }

    /// While a group is referenced the loop runs (as while a referenced callback is enabled).
    pub unsafe fn src_reference(&self, g: u32, on: bool) {
        (self.abi.src_reference)(self.driver, g, on)
    }

    /// Arms `fds` for the wait `token`: Ok(true) when one of them was already ready (nothing is
    /// armed: retry the I/O), Ok(false) when armed.
    pub unsafe fn src_arm(&self, g: u32, fds: &[SrcFd], token: u64) -> std::io::Result<bool> {
        match (self.abi.src_arm)(self.driver, g, fds.as_ptr(), fds.len(), token) {
            r if r < 0 => Err(std::io::Error::from_raw_os_error(-r)),
            r => Ok(r > 0),
        }
    }

    pub unsafe fn src_disarm(&self, fds: &[SrcFd], token: u64) {
        (self.abi.src_disarm)(self.driver, fds.as_ptr(), fds.len(), token)
    }

    /// The source `fd` is about to be closed (a later fd with its number is registered anew).
    pub unsafe fn src_forget(&self, fd: i64) {
        (self.abi.src_forget)(self.driver, fd)
    }

    /// Whether the loop has nothing to do but the caller's callback `except` (0: none): see
    /// `Abi::idle`.
    pub unsafe fn idle(&self, except: u64) -> bool {
        (self.abi.idle)(self.driver, except)
    }
}


/// The fallback: `Revolt\EventLoop`'s static methods through PHP (autoloaded if needed, so it
/// works with the PHP library too).
pub mod php {
    use crate::zend::{call, sys, zval::Zv, zval::Z};

    /// Revolt\EventLoop (autoloaded), or null.
    pub unsafe fn event_loop_class() -> *mut sys::zend_class_entry {
        call::lookup_class_autoload(b"Revolt\\EventLoop")
    }

    /// Revolt\EventLoop::$lcname(...$args): the result (UNDEF with an exception, or when the class
    /// is missing).
    pub unsafe fn call(lcname: &[u8], args: &mut [Z]) -> Zv {
        let ce = event_loop_class();
        if ce.is_null() {
            return Zv::undef();
        }
        call::static_method(ce, lcname, args)
    }
}
