//! Internal\AbstractDriver and the Driver\* classes over the native core.
//!
//! StreamSelectDriver, EvDriver, EventDriver and UvDriver are the same native driver (the
//! reactor in core::backend); the class name keeps isSupported() and getHandle(). PHP classes
//! extending AbstractDriver get the native core too: their activate()/dispatch()/deactivate()/
//! now() are called by it, and their overrides of public methods are honoured where Revolt's
//! own code calls `$this->method()`.

use super::{arg as a, ce, method as m, suspension, Ces, F_PRI as PRI, F_PRO as PRO, F_PUB as PUB, F_STA as STA};
use super::{T_A, T_B, T_CL, T_DCB, T_F, T_I, T_MIX, T_S, T_V};
use crate::core::run::{self, Step, D};
use crate::core::{clock, Core, Desc, DriverKind, Kind, Microtask, Phase};
use crate::zend::class::{self as k, c, Default as Dv, NONE};
use crate::zend::{call, call::Ex, hash, obj, sys, zstr, zval, zval::Zv};
use crate::{zh, zpp};
use std::ptr::null_mut;

pub struct DriverState {
    pub core: Option<Box<Core>>,
}

impl obj::State for DriverState {
    fn new(ce: *mut sys::zend_class_entry) -> Self {
        DriverState { core: Some(Box::new(Core::new(kind_of(ce)))) }
    }
    fn gc(&mut self, add: &mut dyn FnMut(*mut sys::zval)) {
        if let Some(c) = &mut self.core {
            c.gc(add);
        }
    }
}

static mut HANDLERS: *mut sys::zend_object_handlers = null_mut();

/// The Core of a driver object.
#[inline(always)]
pub unsafe fn core<'a>(o: *mut sys::zend_object) -> &'a mut Core {
    obj::state::<DriverState>(o).core.as_mut().unwrap_unchecked()
}

/// Whether `o` is a driver object with the native core (alive).
#[inline(always)]
pub unsafe fn is_native(o: *mut sys::zend_object) -> bool {
    (*o).handlers == HANDLERS as *const _ && !obj::freed(o)
}

/// Whether the native implementation of a Driver method applies to `o` (not overridden in PHP).
#[inline(always)]
pub unsafe fn native_for(o: *mut sys::zend_object, bit: u64) -> bool {
    is_native(o) && core(o).hooks.overrides & bit == 0
}

fn kind_of(ce: *mut sys::zend_class_entry) -> DriverKind {
    let c = super::ce();
    if ce == c.stream_select {
        DriverKind::StreamSelect
    } else if ce == c.ev {
        DriverKind::Ev
    } else if ce == c.event {
        DriverKind::Event
    } else if ce == c.uv {
        DriverKind::Uv
    } else {
        DriverKind::User
    }
}

// Driver methods a PHP subclass may override (bits of Hooks::overrides)
pub const OVR: &[(&str, u64)] = &[
    ("run", 1 << 0),
    ("stop", 1 << 1),
    ("getsuspension", 1 << 2),
    ("isrunning", 1 << 3),
    ("queue", 1 << 4),
    ("defer", 1 << 5),
    ("delay", 1 << 6),
    ("repeat", 1 << 7),
    ("onreadable", 1 << 8),
    ("onwritable", 1 << 9),
    ("onsignal", 1 << 10),
    ("enable", 1 << 11),
    ("cancel", 1 << 12),
    ("disable", 1 << 13),
    ("reference", 1 << 14),
    ("unreference", 1 << 15),
    ("seterrorhandler", 1 << 16),
    ("geterrorhandler", 1 << 17),
    ("gethandle", 1 << 18),
    ("getidentifiers", 1 << 19),
    ("gettype", 1 << 20),
    ("isenabled", 1 << 21),
    ("isreferenced", 1 << 22),
    ("__debuginfo", 1 << 23),
];

pub fn bit(lc: &str) -> u64 {
    OVR.iter().find(|(n, _)| *n == lc).map(|x| x.1).unwrap_or(0)
}

unsafe fn user_method(ce: *mut sys::zend_class_entry, lc: &[u8]) -> Option<*mut sys::zend_function> {
    let f = call::find_method(ce, lc);
    if !f.is_null() && (*f).type_ as u32 == sys::ZEND_USER_FUNCTION {
        Some(f)
    } else {
        None
    }
}

pub unsafe extern "C" fn create_object(ce: *mut sys::zend_class_entry) -> *mut sys::zend_object {
    let o = obj::create::<DriverState>(ce, HANDLERS);
    let c = core(o);
    c.this = o;
    if c.kind == DriverKind::User {
        let h = &mut c.hooks;
        h.activate = user_method(ce, b"activate");
        h.deactivate = user_method(ce, b"deactivate");
        h.dispatch = user_method(ce, b"dispatch");
        h.now = user_method(ce, b"now");
        h.cancel = user_method(ce, b"cancel");
        h.enable = user_method(ce, b"enable");
        h.disable = user_method(ce, b"disable");
        for (n, b) in OVR {
            if user_method(ce, n.as_bytes()).is_some() {
                h.overrides |= b;
            }
        }
        // the abstract methods are implemented in PHP: the hooks are always set then; a
        // getHandle()-only override changes nothing natively
        h.overrides &= !bit("gethandle");
    }
    o
}

// ---------------------------------------------------------------- argument helpers

/// The callback id argument #`n` (a string, coerced as PHP does).
#[inline(always)]
pub unsafe fn id_arg(ex: Ex) -> Option<*mut sys::zend_string> {
    if call::num_args(ex) >= 1 {
        let s = call::str_arg(ex, 0);
        if !s.is_null() {
            return Some(s);
        }
    }
    let mut s: *mut sys::zend_string = null_mut();
    if zpp!(ex, "S", &mut s) {
        Some(s)
    } else {
        None
    }
}

/// The only argument, a Closure.
#[inline(always)]
pub unsafe fn closure_only(ex: Ex) -> Option<*mut sys::zend_object> {
    if call::num_args(ex) >= 1 {
        let o = call::closure_arg(ex, 0);
        if !o.is_null() {
            return Some(o);
        }
    }
    let mut z: *mut sys::zval = null_mut();
    if zpp!(ex, "O", &mut z, sys::zend_ce_closure) {
        Some((*z).value.obj)
    } else {
        None
    }
}

/// (float, Closure)
#[inline(always)]
pub unsafe fn float_closure(ex: Ex) -> Option<(f64, *mut sys::zend_object)> {
    if call::num_args(ex) >= 2 {
        let f = call::arg(ex, 0);
        let o = call::closure_arg(ex, 1);
        if !o.is_null() {
            match zval::ty(f) {
                zval::DOUBLE => return Some(((*f).value.dval, o)),
                zval::LONG => return Some(((*f).value.lval as f64, o)),
                _ => {}
            }
        }
    }
    let mut d = 0f64;
    let mut z: *mut sys::zval = null_mut();
    if zpp!(ex, "dO", &mut d, &mut z, sys::zend_ce_closure) {
        Some((d, (*z).value.obj))
    } else {
        None
    }
}

/// (mixed, Closure)
#[inline(always)]
pub unsafe fn mixed_closure(ex: Ex) -> Option<(*mut sys::zval, *mut sys::zend_object)> {
    if call::num_args(ex) >= 2 {
        let o = call::closure_arg(ex, 1);
        if !o.is_null() {
            return Some((call::arg(ex, 0), o));
        }
    }
    let mut s: *mut sys::zval = null_mut();
    let mut z: *mut sys::zval = null_mut();
    if zpp!(ex, "zO", &mut s, &mut z, sys::zend_ce_closure) {
        Some((s, (*z).value.obj))
    } else {
        None
    }
}

#[inline(always)]
pub unsafe fn ret_id(rv: *mut sys::zval, d: D, id: u64) {
    if let Some(r) = d.c().record(id) {
        zval::write(rv, zval::str_ref(r.id_str));
    }
}

#[inline(always)]
unsafe fn ret_str(rv: *mut sys::zval, s: *mut sys::zend_string) {
    zval::write(rv, zval::str_ref(s));
}

#[inline(always)]
unsafe fn parse_id(s: *mut sys::zend_string) -> u64 {
    crate::core::string_to_id(zstr::bytes(s)).unwrap_or(0)
}

// ---------------------------------------------------------------- AbstractDriver methods

pub unsafe extern "C" fn construct(ex: Ex, _rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    check_php_gc_bug();
    if call::has_exception() {
        return;
    }
    let d = D(call::this(ex));
    let c = d.c();
    let need = match c.kind {
        DriverKind::Ev => Some(("evloop", "EvLoop")),
        DriverKind::Event => Some(("eventbase", "EventBase")),
        _ => None,
    };
    if let Some((lc, name)) = need {
        let cls = call::lookup_class(lc.as_bytes());
        if cls.is_null() {
            call::throw_error(&format!("Class \"{name}\" not found"));
            return;
        }
        let o = call::new_object(cls, &mut []);
        if !o.is_null() {
            d.c().handle.set(zval::obj_owned(o));
        }
    } else if c.kind == DriverKind::Uv {
        if !extension_loaded(b"uv") {
            call::throw_error("Call to undefined function uv_loop_new()");
            return;
        }
        let mut fname = zval::string(b"uv_loop_new");
        let mut rv = Zv::undef();
        sys::_call_user_function_impl(null_mut(), &mut *fname, rv.as_ptr(), 0, null_mut(), null_mut());
        zval::dtor(&mut fname);
        d.c().handle.set(rv.take());
    }
}

/// AbstractDriver::__construct()'s check: PHP 8.2.0 to 8.2.3 have fiber GC bugs (php-src #10496).
unsafe fn check_php_gc_bug() {
    let v = php_version_id();
    if (80200..80204).contains(&v) && crate::zend::getenv("REVOLT_DRIVER_SUPPRESS_ISSUE_10496").is_none_or(|s| s.is_empty() || s == "0") {
        call::throw_error("Your version of PHP is affected by serious garbage collector bugs related to fibers. Please upgrade to a newer version of PHP, i.e. >= 8.1.17 or => 8.2.4");
    }
}

extern "C" {
    fn zend_get_constant_str(name: *const std::ffi::c_char, len: usize) -> *mut sys::zval;
}

unsafe fn php_version_id() -> i64 {
    let z = zend_get_constant_str(c"PHP_VERSION_ID".as_ptr(), 14);
    if z.is_null() || zval::ty(z) != zval::LONG {
        return 0;
    }
    (*z).value.lval
}

pub unsafe fn extension_loaded(lc: &[u8]) -> bool {
    !hash::find_str(std::ptr::addr_of!(sys::module_registry), lc).is_null()
}

pub unsafe extern "C" fn serialize(_ex: Ex, _rv: *mut sys::zval) {
    call::throw_error("Revolt\\EventLoop\\Internal\\AbstractDriver does not support serialization");
}

pub unsafe extern "C" fn unserialize(ex: Ex, _rv: *mut sys::zval) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "a", &mut z) {
        return;
    }
    call::throw_error("Revolt\\EventLoop\\Internal\\AbstractDriver does not support deserialization");
}

pub unsafe extern "C" fn queue(ex: Ex, rv: *mut sys::zval) {
    queue_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn queue_d(ex: Ex, _rv: *mut sys::zval, d: D) {
    let n = call::num_args(ex);
    let o = if n >= 1 { call::closure_arg(ex, 0) } else { null_mut() };
    let o = if o.is_null() {
        let mut z: *mut sys::zval = null_mut();
        let mut rest: *mut sys::zval = null_mut();
        let mut cnt: u32 = 0;
        let mut named: *mut sys::HashTable = null_mut();
        if !zpp!(ex, "O*", &mut z, sys::zend_ce_closure, &mut rest, &mut cnt, &mut named) {
            return;
        }
        let _ = named;
        (*z).value.obj
    } else {
        o
    };
    push_microtask(d, ex, o);
}

/// queue($closure, ...$args) with the frame's arguments (named ones included).
pub unsafe fn push_microtask(d: D, ex: Ex, closure: *mut sys::zend_object) {
    let n = call::num_args(ex) as usize;
    let mut args = Vec::with_capacity(n.saturating_sub(1));
    for i in 1..n {
        args.push(Zv(zval::copy_deref(call::arg(ex, i))));
    }
    let named_ht = call::extra_named(ex);
    let named = if named_ht.is_null() { Zv::undef() } else { Zv(zval::arr_owned(sys::zend_array_dup(named_ht))) };
    d.c().microtasks.push_back(Microtask::Php { closure: Zv(zval::obj_ref(closure)), args, named });
}

pub unsafe extern "C" fn defer(ex: Ex, rv: *mut sys::zval) {
    defer_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn defer_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some(o) = closure_only(ex) else { return };
    let d = d;
    let id = run::defer(d, run::php_cb(o));
    ret_id(rv, d, id);
}

unsafe fn timer(ex: Ex, rv: *mut sys::zval, repeat: bool, d: D) {
    let Some((t, o)) = float_closure(ex) else { return };
    if t < 0.0 {
        call::throw_error(if repeat { "Interval must be greater than or equal to zero" } else { "Delay must be greater than or equal to zero" });
        return;
    }
    if let Some(id) = run::timer(d, t, run::php_cb(o), repeat) {
        ret_id(rv, d, id);
    }
}

pub unsafe extern "C" fn delay(ex: Ex, rv: *mut sys::zval) {
    delay_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn delay_d(ex: Ex, rv: *mut sys::zval, d: D) {
    timer(ex, rv, false, d)
}

pub unsafe extern "C" fn repeat(ex: Ex, rv: *mut sys::zval) {
    repeat_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn repeat_d(ex: Ex, rv: *mut sys::zval, d: D) {
    timer(ex, rv, true, d)
}

unsafe fn on_stream(ex: Ex, rv: *mut sys::zval, writable: bool, d: D) {
    let Some((s, o)) = mixed_closure(ex) else { return };
    let id = run::stream(d, Zv(zval::copy_deref(s)), run::php_cb(o), writable);
    ret_id(rv, d, id);
}

pub unsafe extern "C" fn on_readable(ex: Ex, rv: *mut sys::zval) {
    on_readable_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn on_readable_d(ex: Ex, rv: *mut sys::zval, d: D) {
    on_stream(ex, rv, false, d)
}

pub unsafe extern "C" fn on_writable(ex: Ex, rv: *mut sys::zval) {
    on_writable_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn on_writable_d(ex: Ex, rv: *mut sys::zval, d: D) {
    on_stream(ex, rv, true, d)
}

pub unsafe extern "C" fn on_signal(ex: Ex, rv: *mut sys::zval) {
    on_signal_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn on_signal_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let mut sig: i64 = 0;
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "lO", &mut sig, &mut z, sys::zend_ce_closure) {
        return;
    }
    let d = d;
    if d.c().kind != DriverKind::User && !crate::core::signal::SUPPORTED {
        call::throw(ce().unsupported_feature, "Signal handling requires the pcntl extension");
        return;
    }
    let id = run::signal(d, sig as i32, run::php_cb((*z).value.obj));
    ret_id(rv, d, id);
}

pub unsafe extern "C" fn enable(ex: Ex, rv: *mut sys::zval) {
    enable_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn enable_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some(s) = id_arg(ex) else { return };
    let d = d;
    match run::enable(d, parse_id(s)) {
        Some(true) => ret_str(rv, s),
        Some(false) => {
            zstr::addref(s);
            super::errors::throw_invalid_identifier(s);
        }
        None => {}
    }
}

pub unsafe extern "C" fn disable(ex: Ex, rv: *mut sys::zval) {
    disable_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn disable_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some(s) = id_arg(ex) else { return };
    if run::disable(d, parse_id(s)) {
        ret_str(rv, s);
    }
}

pub unsafe extern "C" fn cancel(ex: Ex, rv: *mut sys::zval) {
    cancel_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn cancel_d(ex: Ex, _rv: *mut sys::zval, d: D) {
    let Some(s) = id_arg(ex) else { return };
    run::cancel(d, parse_id(s));
}

pub unsafe extern "C" fn reference(ex: Ex, rv: *mut sys::zval) {
    reference_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn reference_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some(s) = id_arg(ex) else { return };
    if run::reference(d, parse_id(s)) {
        ret_str(rv, s);
    } else {
        zstr::addref(s);
        super::errors::throw_invalid_identifier(s);
    }
}

pub unsafe extern "C" fn unreference(ex: Ex, rv: *mut sys::zval) {
    unreference_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn unreference_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some(s) = id_arg(ex) else { return };
    run::unreference(d, parse_id(s));
    ret_str(rv, s);
}

/// The record of the id argument, or InvalidCallbackError thrown.
unsafe fn existing(ex: Ex, d: D) -> Option<(D, u64)> {
    let s = id_arg(ex)?;
    let id = parse_id(s);
    if d.c().record(id).is_none() {
        zstr::addref(s);
        super::errors::throw_invalid_identifier(s);
        return None;
    }
    Some((d, id))
}

pub unsafe fn type_case(kind: Kind) -> *mut sys::zend_object {
    let name: &std::ffi::CStr = match kind {
        Kind::Defer => c"Defer",
        Kind::Delay => c"Delay",
        Kind::Repeat => c"Repeat",
        Kind::Readable => c"Readable",
        Kind::Writable => c"Writable",
        Kind::Signal => c"Signal",
    };
    sys::zend_enum_get_case_cstr(ce().callback_type, name.as_ptr())
}

pub unsafe extern "C" fn get_type(ex: Ex, rv: *mut sys::zval) {
    get_type_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn get_type_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some((d, id)) = existing(ex, d) else { return };
    let kind = d.c().record(id).unwrap().kind;
    zval::write(rv, zval::obj_ref(type_case(kind)));
}

pub unsafe extern "C" fn is_enabled(ex: Ex, rv: *mut sys::zval) {
    is_enabled_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn is_enabled_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some((d, id)) = existing(ex, d) else { return };
    zval::write(rv, zval::bool(d.c().record(id).unwrap().enabled));
}

pub unsafe extern "C" fn is_referenced(ex: Ex, rv: *mut sys::zval) {
    is_referenced_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn is_referenced_d(ex: Ex, rv: *mut sys::zval, d: D) {
    let Some((d, id)) = existing(ex, d) else { return };
    zval::write(rv, zval::bool(d.c().record(id).unwrap().referenced));
}

pub unsafe extern "C" fn get_identifiers(ex: Ex, rv: *mut sys::zval) {
    get_identifiers_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn get_identifiers_d(ex: Ex, rv: *mut sys::zval, d: D) {
    if !call::none(ex) {
        return;
    }
    let c = d.c();
    let ids = c.ids();
    let arr = hash::new(ids.len() as u32);
    for id in ids {
        hash::push(arr, zval::str_ref(c.record(id).unwrap().id_str));
    }
    zval::write(rv, zval::arr_owned(arr));
}

pub unsafe extern "C" fn debug_info(ex: Ex, rv: *mut sys::zval) {
    debug_info_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn debug_info_d(ex: Ex, rv: *mut sys::zval, d: D) {
    if !call::none(ex) {
        return;
    }
    let c = d.c();
    let ids = c.ids();
    let arr = hash::new(ids.len() as u32);
    for id in ids {
        let r = c.record(id).unwrap();
        let e = hash::new(3);
        hash::set_str(e, b"type", zval::obj_ref(type_case(r.kind)));
        hash::set_str(e, b"enabled", zval::bool(r.enabled));
        hash::set_str(e, b"referenced", zval::bool(r.referenced));
        hash::set_zstr(arr, r.id_str, zval::arr_owned(e));
    }
    zval::write(rv, zval::arr_owned(arr));
}

pub unsafe extern "C" fn set_error_handler(ex: Ex, rv: *mut sys::zval) {
    set_error_handler_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn set_error_handler_d(ex: Ex, _rv: *mut sys::zval, d: D) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "O!", &mut z, sys::zend_ce_closure) {
        return;
    }
    let v = if z.is_null() { zval::null() } else { zval::copy(z) };
    d.c().error_handler.set(v);
}

pub unsafe extern "C" fn get_error_handler(ex: Ex, rv: *mut sys::zval) {
    get_error_handler_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn get_error_handler_d(ex: Ex, rv: *mut sys::zval, d: D) {
    if !call::none(ex) {
        return;
    }
    zval::write(rv, zval::copy(&d.c().error_handler.0));
}

pub unsafe extern "C" fn get_suspension(ex: Ex, rv: *mut sys::zval) {
    get_suspension_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn get_suspension_d(ex: Ex, rv: *mut sys::zval, d: D) {
    if !call::none(ex) {
        return;
    }
    let s = suspension::get(d);
    if !s.is_null() {
        zval::write(rv, zval::obj_owned(s));
    }
}

pub unsafe extern "C" fn stop(ex: Ex, rv: *mut sys::zval) {
    stop_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn stop_d(ex: Ex, _rv: *mut sys::zval, d: D) {
    if !call::none(ex) {
        return;
    }
    d.c().stopped = true;
}

pub unsafe extern "C" fn is_running(ex: Ex, rv: *mut sys::zval) {
    is_running_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn is_running_d(ex: Ex, rv: *mut sys::zval, d: D) {
    if !call::none(ex) {
        return;
    }
    zval::write(rv, zval::bool(running(d)));
}

/// isRunning(): the loop started and did not finish (Revolt: the loop fiber is running or
/// suspended).
pub fn running(d: D) -> bool {
    d.c().phase != Phase::Idle || run::loop_running(d)
}

pub unsafe extern "C" fn run(ex: Ex, rv: *mut sys::zval) {
    run_d(ex, rv, D(call::this(ex)))
}

pub unsafe fn run_d(ex: Ex, _rv: *mut sys::zval, d: D) {
    if !call::none(ex) {
        return;
    }
    run_impl(d);
}

/// AbstractDriver::run()
pub unsafe fn run_impl(d: D) {
    if run::loop_running(d) {
        call::throw_error("The event loop is already running");
        return;
    }
    if !crate::zend::active_fiber().is_null() {
        call::throw_error("Can't call Revolt\\EventLoop\\Internal\\AbstractDriver::run() within a fiber (i.e., outside of {main})");
        return;
    }
    match run::run_main(d) {
        Step::Done | Step::Exception => {}
        Step::Interrupt(i) => {
            let desc = interrupt_desc(&i);
            if let Some(v) = run::take_interrupt(i) {
                drop(v);
                call::throw_error(&format!("Interrupt from event loop must throw an exception: {desc}"));
            }
        }
    }
}

fn interrupt_desc(i: &crate::core::Interrupt) -> &'static str {
    match i {
        crate::core::Interrupt::Value(_) => "Revolt\\EventLoop\\Internal\\DriverSuspension::resume",
        _ => "Revolt\\EventLoop\\Internal\\DriverSuspension::throw",
    }
}

pub unsafe extern "C" fn enqueue_callback(ex: Ex, _rv: *mut sys::zval) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut z, ce().driver_callback) {
        return;
    }
    let id = super::callbacks::view_id((*z).value.obj);
    if id != 0 {
        D(call::this(ex)).c().enqueue(id);
    }
}

pub unsafe extern "C" fn error(ex: Ex, _rv: *mut sys::zval) {
    let mut cl: *mut sys::zval = null_mut();
    let mut e: *mut sys::zval = null_mut();
    if !zpp!(ex, "OO", &mut cl, sys::zend_ce_closure, &mut e, sys::zend_ce_throwable) {
        return;
    }
    run::error(D(call::this(ex)), Desc::Closure(Zv(zval::copy(cl))), Zv(zval::copy(e)));
}

pub unsafe extern "C" fn private_noop(_ex: Ex, _rv: *mut sys::zval) {}

pub unsafe extern "C" fn is_empty(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    zval::write(rv, zval::bool(D(call::this(ex)).c().live == 0));
}

pub unsafe extern "C" fn callback_id(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let c = D(call::this(ex)).c();
    let s = crate::core::id_to_string(c.next_id);
    c.next_id += 1;
    zval::write(rv, zval::string(&s));
}

// ---------------------------------------------------------------- the native drivers' hooks

/// activate(array $callbacks) of a native driver (reachable through Reflection only).
pub unsafe extern "C" fn native_activate(ex: Ex, _rv: *mut sys::zval) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "a", &mut z) {
        return;
    }
    let mut ids = Vec::new();
    hash::each((*z).value.arr, |_, v| {
        let v = zval::deref(v);
        if zval::ty(v) == zval::OBJECT {
            ids.push(super::callbacks::view_id((*v).value.obj));
        }
        true
    });
    let d = D(call::this(ex));
    for id in ids {
        if let Err(e) = run::activate_one(d, id) {
            e.raise();
            return;
        }
    }
}

pub unsafe extern "C" fn native_deactivate(ex: Ex, _rv: *mut sys::zval) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut z, ce().driver_callback) {
        return;
    }
    run::deactivate_one(D(call::this(ex)), super::callbacks::view_id((*z).value.obj));
}

pub unsafe extern "C" fn native_dispatch(ex: Ex, _rv: *mut sys::zval) {
    let mut b = false;
    if !zpp!(ex, "b", &mut b) {
        return;
    }
    run::dispatch_one(D(call::this(ex)), b);
}

pub unsafe extern "C" fn native_now(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    zval::write(rv, zval::double(clock::now()));
}

pub unsafe extern "C" fn get_handle(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let c = D(call::this(ex)).c();
    if zval::is_undef(&c.handle.0) {
        zval::write(rv, zval::null());
    } else {
        zval::write(rv, zval::copy(&c.handle.0));
    }
}

pub unsafe extern "C" fn destruct(ex: Ex, _rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    // StreamSelectDriver::__destruct(): signal handlers go (the dispositions are restored)
    let d = D(call::this(ex));
    let ids: Vec<u64> = d.c().callbacks.values().filter(|r| r.kind == Kind::Signal && r.active).map(|r| r.id).collect();
    for id in ids {
        run::deactivate_one(d, id);
    }
}

pub unsafe extern "C" fn is_supported_ev(_ex: Ex, rv: *mut sys::zval) {
    zval::write(rv, zval::bool(extension_loaded(b"ev")));
}

pub unsafe extern "C" fn is_supported_event(_ex: Ex, rv: *mut sys::zval) {
    zval::write(rv, zval::bool(extension_loaded(b"event")));
}

pub unsafe extern "C" fn is_supported_uv(_ex: Ex, rv: *mut sys::zval) {
    zval::write(rv, zval::bool(extension_loaded(b"uv")));
}

pub unsafe extern "C" fn private_float(_ex: Ex, rv: *mut sys::zval) {
    zval::write(rv, zval::double(0.0));
}

pub unsafe extern "C" fn private_int(_ex: Ex, rv: *mut sys::zval) {
    zval::write(rv, zval::long(0));
}

/// The handler of an AbstractDriver method by name.
pub fn handler(name: &'static str) -> k::Handler {
    match name {
        "run" => zh!(run),
        "stop" => zh!(stop),
        "getSuspension" => zh!(get_suspension),
        "isRunning" => zh!(is_running),
        "queue" => zh!(queue),
        "defer" => zh!(defer),
        "delay" => zh!(delay),
        "repeat" => zh!(repeat),
        "onReadable" => zh!(on_readable),
        "onWritable" => zh!(on_writable),
        "onSignal" => zh!(on_signal),
        "enable" => zh!(enable),
        "cancel" => zh!(cancel),
        "disable" => zh!(disable),
        "reference" => zh!(reference),
        "unreference" => zh!(unreference),
        "setErrorHandler" => zh!(set_error_handler),
        "getErrorHandler" => zh!(get_error_handler),
        "getHandle" => zh!(get_handle),
        "getIdentifiers" => zh!(get_identifiers),
        "getType" => zh!(get_type),
        "isEnabled" => zh!(is_enabled),
        "isReferenced" => zh!(is_referenced),
        "__debugInfo" => zh!(debug_info),
        _ => None,
    }
}

/// The four native driver classes and EventLoop::setDriver()'s garbage-collection driver.
pub unsafe fn register_drivers(ces: &mut Ces) {
    HANDLERS = obj::handlers::<DriverState>();
    let hooks = || {
        vec![
            m("__construct", zh!(construct), PUB, NONE, vec![]),
            m("__destruct", zh!(destruct), PUB, NONE, vec![]),
            m("activate", zh!(native_activate), PRO, T_V, vec![a("callbacks", T_A)]),
            m("deactivate", zh!(native_deactivate), PRO, T_V, vec![a("callback", T_DCB)]),
            m("dispatch", zh!(native_dispatch), PRO, T_V, vec![a("blocking", T_B)]),
            m("now", zh!(native_now), PRO, T_F, vec![]),
        ]
    };
    let ad = ces.abstract_driver;
    let pr = k::PRIVATE;

    let mut ss = hooks();
    ss.extend(vec![
        m("onSignal", zh!(on_signal), PUB, T_S, vec![a("signal", T_I), a("closure", T_CL)]),
        m("getHandle", zh!(get_handle), PUB, T_MIX, vec![]),
        m("selectStreams", zh!(private_noop), PRI, T_V, vec![a("read", T_A), a("write", T_A), a("timeout", T_F)]),
        m("getTimeout", zh!(private_float), PRI, T_F, vec![]),
        m("handleSignal", zh!(private_noop), PRI, T_V, vec![a("signal", T_I)]),
    ]);
    ces.stream_select = k::register_class("Revolt\\EventLoop\\Driver\\StreamSelectDriver", ss, ad, k::FINAL);
    let s = ces.stream_select;
    for n in ["readStreams", "readCallbacks", "writeStreams", "writeCallbacks", "signalCallbacks"] {
        k::prop(s, n, T_A, pr, Dv::EmptyArray);
    }
    k::prop(s, "timerQueue", c("Revolt\\EventLoop\\Internal\\TimerQueue", false), pr | k::READONLY, Dv::Undef);
    k::prop(s, "signalQueue", c("SplQueue", false), pr | k::READONLY, Dv::Undef);
    k::prop(s, "signalHandling", T_B, pr, Dv::Undef);
    k::prop(s, "streamSelectErrorHandler", T_CL, pr | k::READONLY, Dv::Undef);
    k::prop(s, "streamSelectIgnoreResult", T_B, pr, Dv::Bool(false));

    for (name, handle, supported) in [
        ("Revolt\\EventLoop\\Driver\\EvDriver", "EvLoop", zh!(is_supported_ev)),
        ("Revolt\\EventLoop\\Driver\\EventDriver", "EventBase", zh!(is_supported_event)),
    ] {
        let mut ms = hooks();
        ms.extend(vec![
            m("isSupported", supported, PUB | STA, T_B, vec![]),
            m("cancel", zh!(cancel), PUB, T_V, vec![a("callbackId", T_S)]),
            m("run", zh!(run), PUB, T_V, vec![]),
            m("stop", zh!(stop), PUB, T_V, vec![]),
            m("getHandle", zh!(get_handle), PUB, c(handle, false), vec![]),
        ]);
        let cls = k::register_class(name, ms, ad, k::FINAL);
        k::prop(cls, "activeSignals", crate::zend::class::t(k::T_ARRAY | k::T_NULL), pr | k::STATIC, Dv::Null);
        k::prop(cls, "handle", c(handle, false), pr, Dv::Undef);
        k::prop(cls, "events", T_A, pr, Dv::EmptyArray);
        k::prop(cls, "ioCallback", T_CL, pr | k::READONLY, Dv::Undef);
        k::prop(cls, "timerCallback", T_CL, pr | k::READONLY, Dv::Undef);
        k::prop(cls, "signalCallback", T_CL, pr | k::READONLY, Dv::Undef);
        k::prop(cls, "signals", T_A, pr, Dv::EmptyArray);
        if handle == "EvLoop" {
            ces.ev = cls;
        } else {
            ces.event = cls;
        }
    }

    let mut uv = hooks();
    uv.extend(vec![
        m("isSupported", zh!(is_supported_uv), PUB | STA, T_B, vec![]),
        m("cancel", zh!(cancel), PUB, T_V, vec![a("callbackId", T_S)]),
        m("getHandle", zh!(get_handle), PUB, T_MIX, vec![]),
        m("getStreamCallbackFlags", zh!(private_int), PRI, T_I, vec![a("callback", c("Revolt\\EventLoop\\Internal\\StreamCallback", false))]),
    ]);
    ces.uv = k::register_class("Revolt\\EventLoop\\Driver\\UvDriver", uv, ad, k::FINAL);
    let u = ces.uv;
    k::prop(u, "handle", NONE, pr, Dv::Null);
    for n in ["events", "uvCallbacks", "streams"] {
        k::prop(u, n, T_A, pr, Dv::EmptyArray);
    }
    for n in ["ioCallback", "timerCallback", "signalCallback"] {
        k::prop(u, n, T_CL, pr | k::READONLY, Dv::Undef);
    }

    for cls in [ces.stream_select, ces.ev, ces.event, ces.uv] {
        (*cls).__bindgen_anon_2.create_object = Some(create_object);
    }
    run::minit(ad);
}
