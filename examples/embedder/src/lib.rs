//! revolt_embed_example: a test extension embedding revolt-rs, as a real embedder (e.g. a
//! database driver) would. It registers the Revolt classes when nobody else did, drives the loop
//! natively through `revolt_rs::embed::NativeLoop` when it can, and through PHP-level calls to
//! `Revolt\EventLoop` otherwise. Functions:
//!
//! - `revolt_embed_example_mode(): string` — "native" (this copy's own table), "native-foreign"
//!   (the table of the copy that registered the classes), "php" (the PHP-call fallback);
//! - `revolt_embed_example_owner(): bool` — whether this copy registered the classes;
//! - `revolt_embed_example_sleep(float $seconds): mixed` — suspends the current fiber or {main}
//!   on a timer (the value the timer resumes with: "native" or "php", the path taken);
//! - `revolt_embed_example_wait_readable(resource $stream): string` — suspends until the stream's
//!   OS handle is readable (its PHP read buffer is not looked at), returns the path taken;
//! - `revolt_embed_example_delay(float $seconds): ?string` — a native timer that does nothing,
//!   its Revolt callback id (null when there is no native path);
//! - `revolt_embed_example_released(): int` — how many native callbacks were released so far;
//! - `revolt_embed_example_src_wait(resource $stream, bool $writable = false): string` — waits
//!   for the stream's OS handle as a source (edge-triggered, outside the callback records):
//!   "native" once a flush resumed it, "ready" when readiness seen earlier ended the wait at once,
//!   "php" through the fallback (onReadable/onWritable);
//! - `revolt_embed_example_src_forget(resource $stream): void` — forgets the source (before the
//!   stream is closed);
//! - `revolt_embed_example_src_stats(): array` — ["takes" => n, "flushes" => n] so far;
//! - `revolt_embed_example_idle(): ?bool` — whether the loop has nothing to do (null: no native
//!   path).
//!
//! `REVOLT_EMBED_EXAMPLE_NO_REGISTER=1` in the environment makes MINIT skip class registration
//! (to test the fallback against the PHP library).
#![cfg_attr(windows, feature(abi_vectorcall))]
#![allow(clippy::missing_safety_doc)]

use revolt_rs::embed::{self, NativeLoop};
use revolt_rs::module;
use revolt_rs::zend::class::{self as k, a, Method};
use revolt_rs::zend::{call, hash, stream, sys, zval, zval::Zv};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::OnceLock;

thread_local! {
    static RELEASED: Cell<i64> = const { Cell::new(0) };
}

unsafe extern "C" fn minit(_type: i32, module_number: i32) -> sys::zend_result {
    let register = std::env::var_os("REVOLT_EMBED_EXAMPLE_NO_REGISTER").is_none_or(|v| v != "1");
    module::minit_with(module_number, register);
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn mshutdown(_type: i32, _module_number: i32) -> sys::zend_result {
    module::mshutdown();
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn rinit(_type: i32, _module_number: i32) -> sys::zend_result {
    module::rinit();
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn rshutdown(_type: i32, _module_number: i32) -> sys::zend_result {
    module::rshutdown();
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn minfo(_module: *mut sys::zend_module_entry) {
    module::minfo();
}

/// What a pending native wait holds: the owner's table and the suspension to resume (borrowed:
/// the callback's `held` reference keeps it alive).
struct Wait {
    abi: &'static embed::Abi,
    suspension: *mut sys::zend_object,
}

unsafe extern "C" fn wait_fire(data: *mut c_void, _id: u64) {
    let w = &*(data as *const Wait);
    let mut v = zval::string(b"native");
    (w.abi.resume)(w.suspension, &mut *v);
    zval::dtor(&mut v);
}

unsafe extern "C" fn wait_release(data: *mut c_void) {
    drop(Box::from_raw(data as *mut Wait));
    RELEASED.with(|r| r.set(r.get() + 1));
}

unsafe extern "C" fn noop_fire(_data: *mut c_void, _id: u64) {}

unsafe extern "C" fn noop_release(_data: *mut c_void) {
    RELEASED.with(|r| r.set(r.get() + 1));
}

/// Suspends on a native callback registered by `register(loop, data, held)`; the callback is
/// cancelled afterwards (a readable watcher fires until then). The value goes to `rv`.
///
/// The suspension (and through it the driver) stays alive through `hold`, where the cycle
/// collector sees it, not through a reference of this frame: so a fiber suspended here whose
/// driver became garbage is collected (its destruction unwinds this frame: `suspend` returns
/// with the unwind exception, and the objects are still there for the cancel).
unsafe fn native_wait(nl: NativeLoop, rv: *mut sys::zval, register: impl FnOnce(&NativeLoop, *mut c_void, *mut sys::zend_object) -> u64) {
    let s = nl.suspension();
    if s.is_null() {
        return;
    }
    nl.hold(s);
    let mut own = zval::obj_owned(s);
    zval::dtor(&mut own);
    let data = Box::into_raw(Box::new(Wait { abi: nl.abi, suspension: s })) as *mut c_void;
    let id = register(&nl, data, s);
    if id != 0 {
        let value = nl.suspend(s);
        nl.cancel(id);
        if let Some(mut v) = value {
            zval::write(rv, v.take());
        }
    }
    nl.unhold();
}

/// `Closure::fromCallable([$object, $method])`
unsafe fn method_closure(o: *mut sys::zend_object, method: &[u8]) -> Zv {
    let arr = hash::new(2);
    hash::push(arr, zval::obj_ref(o));
    hash::push(arr, zval::string(method));
    let mut args = [zval::arr_owned(arr)];
    let r = call::static_method(sys::zend_ce_closure, b"fromcallable", &mut args);
    zval::dtor(&mut args[0]);
    r
}

/// The fallback: `$s = EventLoop::getSuspension(); $id = EventLoop::$method(...$args,
/// $s->resume(...)); $s->suspend(); EventLoop::cancel($id); return "php";`
unsafe fn php_wait(rv: *mut sys::zval, method: &[u8], mut args: Vec<zval::Z>) {
    let s = embed::php::call(b"getsuspension", &mut []);
    if s.ty() != zval::OBJECT {
        args.iter_mut().for_each(|z| zval::dtor(z));
        return;
    }
    args.push(method_closure(s.obj(), b"resume").take());
    let mut id = embed::php::call(method, &mut args);
    args.iter_mut().for_each(|z| zval::dtor(z));
    if id.ty() != zval::STRING {
        return;
    }
    drop(call::method(s.obj(), b"suspend", &mut []));
    if call::has_exception() && call::exception_is_unwind() {
        return; // exit() or the fiber's destruction: no more PHP calls
    }
    // cancel even after an exception: keep it aside meanwhile
    let ex = call::take_exception();
    let mut cancel_args = [id.take()];
    drop(embed::php::call(b"cancel", &mut cancel_args));
    zval::dtor(&mut cancel_args[0]);
    if !ex.is_null() {
        call::rethrow(ex);
    } else if !call::has_exception() {
        zval::write(rv, zval::string(b"php"));
    }
}

fn mode() -> &'static str {
    if NativeLoop::current().is_none() {
        "php"
    } else if embed::is_foreign() {
        "native-foreign"
    } else {
        "native"
    }
}

unsafe extern "C" fn fn_mode(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    if !revolt_rs::zpp!(ex, "") {
        return;
    }
    let m = mode();
    if !call::has_exception() {
        zval::write(rv, zval::string(m.as_bytes()));
    }
}

unsafe extern "C" fn fn_owner(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    if !revolt_rs::zpp!(ex, "") {
        return;
    }
    zval::write(rv, zval::bool(module::is_owner()));
}

unsafe extern "C" fn fn_sleep(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    let mut secs = 0.0f64;
    if !revolt_rs::zpp!(ex, "d", &mut secs) {
        return;
    }
    match NativeLoop::current() {
        Some(nl) => native_wait(nl, rv, |nl, data, s| nl.delay(secs, wait_fire, data, Some(wait_release), s)),
        None if call::has_exception() => {}
        None => php_wait(rv, b"delay", vec![zval::double(secs)]),
    }
}

unsafe extern "C" fn fn_wait_readable(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    let mut z: *mut sys::zval = std::ptr::null_mut();
    if !revolt_rs::zpp!(ex, "r", &mut z) {
        return;
    }
    let Some(h) = stream::handle(z) else {
        call::throw(sys::zend_ce_type_error, "revolt_embed_example_wait_readable(): Argument #1 ($stream) must be a stream with an OS handle");
        return;
    };
    match NativeLoop::current() {
        Some(nl) => native_wait(nl, rv, |nl, data, s| nl.on_readable(h.fd, h.socket, wait_fire, data, Some(wait_release), s)),
        None if call::has_exception() => {}
        None => php_wait(rv, b"onreadable", vec![zval::copy(z)]),
    }
}

unsafe extern "C" fn fn_delay(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    let mut secs = 0.0f64;
    if !revolt_rs::zpp!(ex, "d", &mut secs) {
        return;
    }
    let Some(nl) = NativeLoop::current() else {
        zval::write(rv, zval::null());
        return;
    };
    let id = nl.delay(secs, noop_fire, std::ptr::null_mut(), Some(noop_release), std::ptr::null_mut());
    if id != 0 {
        zval::write(rv, zval::string(&revolt_rs::core::id_to_string(id)));
    }
}

unsafe extern "C" fn fn_released(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    if !revolt_rs::zpp!(ex, "") {
        return;
    }
    zval::write(rv, zval::long(RELEASED.with(|r| r.get())));
}

/// The sources of this thread: one group on the current driver; the waits pending on it (token
/// -> suspension, borrowed: held by the waiting frame) and those `take` accepted.
struct Src {
    driver: *mut sys::zend_object,
    group: u32,
    abi: Option<&'static embed::Abi>,
    next_token: u64,
    waiting: Vec<(u64, *mut sys::zend_object)>,
    taken: Vec<*mut sys::zend_object>,
    takes: i64,
    flushes: i64,
}

thread_local! {
    static SRC: RefCell<Src> = const { RefCell::new(Src { driver: std::ptr::null_mut(), group: 0, abi: None, next_token: 1, waiting: Vec::new(), taken: Vec::new(), takes: 0, flushes: 0 }) };
}

/// During the loop's poll: the wait `token` is resumed at the flush if it is still pending.
unsafe extern "C" fn src_take(_data: *mut c_void, token: u64) -> bool {
    SRC.with_borrow_mut(|s| {
        let Some(i) = s.waiting.iter().position(|w| w.0 == token) else { return false };
        let (_, susp) = s.waiting.swap_remove(i);
        s.taken.push(susp);
        s.takes += 1;
        true
    })
}

/// After the poll: the waits taken are resumed (one call for all of them).
unsafe extern "C" fn src_flush(_data: *mut c_void) {
    let (abi, taken) = SRC.with_borrow_mut(|s| {
        s.flushes += 1;
        (s.abi, std::mem::take(&mut s.taken))
    });
    let Some(abi) = abi else { return };
    for susp in taken {
        let mut v = zval::string(b"native");
        (abi.resume)(susp, &mut *v);
        zval::dtor(&mut v);
    }
}

/// The group on the current driver (made on first use, and for a new driver: each request has
/// its own).
unsafe fn src_group(nl: &NativeLoop) -> u32 {
    let (d, g) = SRC.with_borrow(|s| (s.driver, s.group));
    if d == nl.driver && g != 0 {
        return g;
    }
    let g = nl.src_group(src_take, src_flush, std::ptr::null_mut());
    SRC.with_borrow_mut(|s| {
        s.driver = nl.driver;
        s.group = g;
        s.abi = Some(nl.abi);
    });
    g
}

unsafe extern "C" fn fn_src_wait(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    let mut z: *mut sys::zval = std::ptr::null_mut();
    let mut writable = false;
    if !revolt_rs::zpp!(ex, "r|b", &mut z, &mut writable) {
        return;
    }
    let Some(h) = stream::handle(z) else {
        call::throw(sys::zend_ce_type_error, "revolt_embed_example_src_wait(): Argument #1 ($stream) must be a stream with an OS handle");
        return;
    };
    let Some(nl) = NativeLoop::current() else {
        if !call::has_exception() {
            php_wait(rv, if writable { b"onwritable" } else { b"onreadable" }, vec![zval::copy(z)]);
        }
        return;
    };
    let g = src_group(&nl);
    if g == 0 {
        return;
    }
    let token = SRC.with_borrow_mut(|s| {
        s.next_token += 1;
        s.next_token
    });
    let fds = [embed::SrcFd { fd: h.fd, events: if writable { embed::SRC_WRITE } else { embed::SRC_READ } as u32 }];
    match nl.src_arm(g, &fds, token) {
        Err(e) => {
            call::throw(sys::zend_ce_exception, &format!("src_arm: {e}"));
            return;
        }
        Ok(true) => {
            zval::write(rv, zval::string(b"ready"));
            return;
        }
        Ok(false) => {}
    }
    let susp = nl.suspension();
    if susp.is_null() {
        nl.src_disarm(&fds, token);
        return;
    }
    nl.hold(susp);
    let mut own = zval::obj_owned(susp);
    zval::dtor(&mut own);
    let first = SRC.with_borrow_mut(|s| {
        s.waiting.push((token, susp));
        s.waiting.len() == 1
    });
    if first {
        nl.src_reference(g, true);
    }
    let value = nl.suspend(susp);
    nl.src_disarm(&fds, token);
    let last = SRC.with_borrow_mut(|s| {
        s.waiting.retain(|w| w.0 != token);
        s.waiting.is_empty()
    });
    if last {
        nl.src_reference(g, false);
    }
    nl.unhold();
    if let Some(mut v) = value {
        zval::write(rv, v.take());
    }
}

unsafe extern "C" fn fn_src_forget(ex: *mut sys::zend_execute_data, _rv: *mut sys::zval) {
    let mut z: *mut sys::zval = std::ptr::null_mut();
    if !revolt_rs::zpp!(ex, "r", &mut z) {
        return;
    }
    if let (Some(h), Some(nl)) = (stream::handle(z), NativeLoop::current()) {
        nl.src_forget(h.fd);
    }
}

unsafe extern "C" fn fn_src_stats(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    if !revolt_rs::zpp!(ex, "") {
        return;
    }
    let (t, f) = SRC.with_borrow(|s| (s.takes, s.flushes));
    let arr = hash::new(2);
    hash::set_str(arr, b"takes", zval::long(t));
    hash::set_str(arr, b"flushes", zval::long(f));
    zval::write(rv, zval::arr_owned(arr));
}

unsafe extern "C" fn fn_idle(ex: *mut sys::zend_execute_data, rv: *mut sys::zval) {
    if !revolt_rs::zpp!(ex, "") {
        return;
    }
    match NativeLoop::current() {
        Some(nl) => zval::write(rv, zval::bool(nl.idle(0))),
        None => zval::write(rv, zval::null()),
    }
}

fn functions() -> *const sys::zend_function_entry {
    let f = |name, handler, ret, args| -> Method { k::m(name, handler, 0, ret, args) };
    k::functions(vec![
        f("revolt_embed_example_mode", revolt_rs::zh!(fn_mode), k::t(k::T_STRING), vec![]),
        f("revolt_embed_example_owner", revolt_rs::zh!(fn_owner), k::t(k::T_BOOL), vec![]),
        f("revolt_embed_example_sleep", revolt_rs::zh!(fn_sleep), k::t(k::T_MIXED), vec![a("seconds", k::t(k::T_FLOAT))]),
        f("revolt_embed_example_wait_readable", revolt_rs::zh!(fn_wait_readable), k::t(k::T_MIXED), vec![a("stream", k::t(k::T_MIXED))]),
        f("revolt_embed_example_delay", revolt_rs::zh!(fn_delay), k::t(k::T_STRING | k::T_NULL), vec![a("seconds", k::t(k::T_FLOAT))]),
        f("revolt_embed_example_released", revolt_rs::zh!(fn_released), k::t(k::T_INT), vec![]),
        f("revolt_embed_example_src_wait", revolt_rs::zh!(fn_src_wait), k::t(k::T_STRING), vec![a("stream", k::t(k::T_MIXED)), revolt_rs::zend::class::ad("writable", k::t(k::T_BOOL), "false")]),
        f("revolt_embed_example_src_forget", revolt_rs::zh!(fn_src_forget), k::t(k::T_VOID), vec![a("stream", k::t(k::T_MIXED))]),
        f("revolt_embed_example_src_stats", revolt_rs::zh!(fn_src_stats), k::t(k::T_ARRAY), vec![]),
        f("revolt_embed_example_idle", revolt_rs::zh!(fn_idle), k::t(k::T_BOOL | k::T_NULL), vec![]),
    ])
}

struct Entry(sys::zend_module_entry);
unsafe impl Send for Entry {}
unsafe impl Sync for Entry {}

static ENTRY: OnceLock<Entry> = OnceLock::new();

#[no_mangle]
pub extern "C" fn get_module() -> *mut sys::zend_module_entry {
    let e = ENTRY.get_or_init(|| {
        let mut e = module::module_entry(c"revolt_embed_example", c"0.1.0", minit, mshutdown, rinit, rshutdown, minfo);
        e.functions = functions();
        Entry(e)
    });
    &e.0 as *const sys::zend_module_entry as *mut sys::zend_module_entry
}
