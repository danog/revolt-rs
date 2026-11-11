//! Internal\DriverSuspension: suspends `{main}` (running the loop meanwhile) or the current
//! fiber, and resumes it from a callback.

use super::{arg as a, arg_default as ad, ce, method as m, Ces, F_PRI as PRI, F_PUB as PUB};
use super::{T_A, T_CL, T_MIX, T_S, T_THR, T_V};
use crate::core::run::{self, Step, D};
use crate::core::{Interrupt, Microtask};
use crate::zend::class::{self as k, c, Default as Dv, NONE};
use crate::zend::{self, call, call::Ex, fiber, hash, obj, sys, zstr, zval, zval::Zv};
use crate::{zh, zpp};
use std::ptr::null_mut;

pub struct SuspState {
    /// the driver (a reference), UNDEF for an object made with `new`
    driver: Zv,
    /// the fiber (weak: alive while the driver's table maps it to this suspension); null: {main}
    fiber: *mut sys::zend_object,
    suspended_fiber: Zv,
    error: Zv,
    pending: bool,
    dead_main: bool,
}

impl obj::State for SuspState {
    fn new(_ce: *mut sys::zend_class_entry) -> Self {
        SuspState { driver: Zv::undef(), fiber: null_mut(), suspended_fiber: Zv::undef(), error: Zv::null(), pending: false, dead_main: false }
    }
    fn gc(&mut self, add: &mut dyn FnMut(*mut sys::zval)) {
        add(&mut *self.driver.0);
        add(&mut *self.suspended_fiber.0);
        add(&mut *self.error.0);
    }
}

impl Drop for SuspState {
    fn drop(&mut self) {
        // unregister (the driver may be gone already in a GC run or at shutdown)
        let d = self.driver.obj();
        if d.is_null() || unsafe { obj::freed(d) || !super::driver::is_native(d) } {
            return;
        }
        let me = self_obj(self);
        let c = D(d).c();
        if self.fiber.is_null() {
            if c.main_suspension == me {
                c.main_suspension = null_mut();
            }
        } else {
            c.suspensions.remove(self.fiber, me as *mut std::ffi::c_void);
        }
    }
}

static mut HANDLERS: *mut sys::zend_object_handlers = null_mut();

/// The object of a state (the state is right before it).
fn self_obj(s: &mut SuspState) -> *mut sys::zend_object {
    unsafe { (s as *mut SuspState as *mut u8).add(std::mem::offset_of!(obj::Native<SuspState>, std)) as *mut sys::zend_object }
}

#[inline(always)]
unsafe fn st<'a>(o: *mut sys::zend_object) -> &'a mut SuspState {
    obj::state::<SuspState>(o)
}

unsafe extern "C" fn create_object(cls: *mut sys::zend_class_entry) -> *mut sys::zend_object {
    obj::create::<SuspState>(cls, HANDLERS)
}

pub unsafe fn register(ces: &mut Ces) {
    HANDLERS = obj::handlers::<SuspState>();
    let cls = k::register_class(
        "Revolt\\EventLoop\\Internal\\DriverSuspension",
        vec![
            m("__construct", zh!(construct), PUB, NONE, vec![a("run", T_CL), a("queue", T_CL), a("interrupt", T_CL), a("suspensions", c("WeakMap", false))]),
            m("resume", zh!(resume), PUB, T_V, vec![ad("value", T_MIX, "null")]),
            m("suspend", zh!(suspend), PUB, T_MIX, vec![]),
            m("throw", zh!(throw), PUB, T_V, vec![a("throwable", T_THR)]),
            m("formatStacktrace", zh!(format_stacktrace), PRI, T_S, vec![a("trace", T_A)]),
        ],
        null_mut(),
        k::FINAL | k::NOT_SERIALIZABLE,
    );
    k::implements(cls, ces.suspension);
    (*cls).__bindgen_anon_2.create_object = Some(create_object);
    let pr = k::PRIVATE;
    k::prop(cls, "suspendedFiber", c("Fiber", true), pr, Dv::Null);
    k::prop(cls, "fiberRef", c("WeakReference", true), pr | k::READONLY, Dv::Undef);
    k::prop(cls, "error", c("Error", true), pr, Dv::Null);
    k::prop(cls, "pending", crate::zend::class::t(k::T_BOOL), pr, Dv::Bool(false));
    k::prop(cls, "deadMain", crate::zend::class::t(k::T_BOOL), pr, Dv::Bool(false));
    k::prop(cls, "run", T_CL, pr | k::READONLY, Dv::Undef);
    k::prop(cls, "queue", T_CL, pr | k::READONLY, Dv::Undef);
    k::prop(cls, "interrupt", T_CL, pr | k::READONLY, Dv::Undef);
    k::prop(cls, "suspensions", c("WeakMap", false), pr | k::READONLY, Dv::Undef);
    ces.driver_suspension = cls;
}

/// getSuspension(): the suspension of the current fiber (or `{main}`), the same object while it
/// lives. A new reference.
pub unsafe fn get(d: D) -> *mut sys::zend_object {
    let f = zend::active_fiber();
    let c = d.c();
    let found = if f.is_null() { c.main_suspension } else { c.suspensions.get(f) as *mut sys::zend_object };
    if !found.is_null() {
        (*found).gc.refcount += 1;
        return found;
    }
    let o = create_object(ce().driver_suspension);
    let s = st(o);
    s.driver = Zv(zval::obj_ref(d.0));
    s.fiber = f;
    let c = d.c();
    if f.is_null() {
        c.main_suspension = o;
    } else {
        c.suspensions.add(f, o as *mut _);
    }
    o
}

/// The driver of a suspension, or an Error thrown.
unsafe fn driver_of(s: &SuspState) -> Option<D> {
    let d = s.driver.obj();
    if d.is_null() || !super::driver::is_native(d) {
        call::throw_error("This Suspension is not attached to an event loop driver");
        return None;
    }
    Some(D(d))
}

/// Whether the suspension's fiber is alive (its registration in the driver's table holds).
unsafe fn fiber_alive(o: *mut sys::zend_object, s: &SuspState, d: D) -> bool {
    !s.fiber.is_null() && d.c().suspensions.get(s.fiber) == o as *mut _
}

unsafe extern "C" fn construct(ex: Ex, _rv: *mut sys::zval) {
    let (mut r, mut q, mut i, mut w): (*mut sys::zval, *mut sys::zval, *mut sys::zval, *mut sys::zval) = (null_mut(), null_mut(), null_mut(), null_mut());
    if !zpp!(ex, "OOOO", &mut r, sys::zend_ce_closure, &mut q, sys::zend_ce_closure, &mut i, sys::zend_ce_closure, &mut w, super::class_lc(b"weakmap")) {
        return;
    }
    call::throw_error("Revolt\\EventLoop\\Internal\\DriverSuspension objects are created by the event loop driver (getSuspension())");
}

unsafe extern "C" fn resume(ex: Ex, _rv: *mut sys::zval) {
    let mut v: *mut sys::zval = null_mut();
    if call::num_args(ex) > 0 && !zpp!(ex, "|z", &mut v) {
        return;
    }
    let value = if v.is_null() { Zv::null() } else { Zv(zval::copy_deref(v)) };
    wake(call::this(ex), value, false);
}

unsafe extern "C" fn throw(ex: Ex, _rv: *mut sys::zval) {
    let mut e: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut e, sys::zend_ce_throwable) {
        return;
    }
    wake(call::this(ex), Zv(zval::copy(e)), true);
}

/// resume()/throw()
pub unsafe fn wake(o: *mut sys::zend_object, value: Zv, throw: bool) {
    let s = st(o);
    if s.dead_main {
        // spurious resumes of a dead {main} suspension are ignored
        return;
    }
    let Some(d) = driver_of(s) else { return };
    if !s.pending {
        if !zval::is_null(&s.error.0) {
            call::rethrow(zval::copy(&s.error.0).value.obj);
        } else {
            call::throw_error(if throw { "Must call suspend() before calling throw()" } else { "Must call suspend() before calling resume()" });
        }
        return;
    }
    s.pending = false;
    if s.fiber.is_null() {
        // suspend the loop to {main}
        d.c().interrupt = Some(if throw { Interrupt::Throw(value) } else { Interrupt::Value(value) });
        return;
    }
    if fiber_alive(o, s, d) {
        d.c().microtasks.push_back(Microtask::Resume { fiber: Zv(zval::obj_ref(s.fiber)), value, throw });
    }
    // a fiber that died while suspended (destroyed by the GC) is not resumed
}

/// $suspension->suspend(): the value (UNDEF with an exception pending).
pub unsafe fn suspend_value(o: *mut sys::zend_object) -> Zv {
    let s = st(o);
    if s.dead_main {
        call::throw_error("Suspension cannot be suspended after an uncaught exception is thrown from the event loop");
        return Zv::undef();
    }
    if s.pending {
        call::throw_error("Must call resume() or throw() before calling suspend() again");
        return Zv::undef();
    }
    let Some(d) = driver_of(s) else { return Zv::undef() };
    let cur = zend::active_fiber();
    let mine = if s.fiber.is_null() || fiber_alive(o, s, d) { s.fiber } else { null_mut() };
    if mine != cur || (!s.fiber.is_null() && mine.is_null()) {
        call::throw_error("Must not call suspend() from another fiber");
        return Zv::undef();
    }
    s.pending = true;
    s.error.set(zval::null());

    if !cur.is_null() {
        // awaiting from within a fiber
        s.suspended_fiber.set(zval::obj_ref(cur));
        let mut n = zval::null();
        let v = fiber::suspend(&mut n);
        // the state may have been freed meanwhile only with the object, which the frame holds
        let s = st(o);
        if call::has_exception() {
            let e = (*zend::eg()).exception;
            let fe = fiber::fiber_error_ce();
            if !fe.is_null() && call::instance_of((*e).ce, fe) {
                s.pending = false;
                s.suspended_fiber.set(zval::null());
                s.error.set(zval::obj_ref(e));
            }
            return Zv::undef();
        }
        s.suspended_fiber.set(zval::null());
        return v;
    }

    // awaiting from {main}
    if run::loop_running(d) {
        s.pending = false;
        call::throw_error("The event loop is already running");
        return Zv::undef();
    }
    let result = match run::run_main(d) {
        Step::Exception => return Zv::undef(),
        Step::Done => None,
        Step::Interrupt(i) => Some(i),
    };
    let s = st(o);
    if s.pending {
        // a dead {main} suspension from now on
        s.dead_main = true;
        let c = d.c();
        if c.main_suspension == o {
            c.main_suspension = null_mut();
        }
        if let Some(i) = result {
            // unwrap an uncaught exception of the loop
            match run::take_interrupt(i) {
                None => return Zv::undef(),
                Some(v) => drop(v),
            }
        }
        zend::gc_collect_cycles();
        let info = pending_traces(d);
        call::throw_error(&format!("Event loop terminated without resuming the current suspension (the cause is either a fiber deadlock, or an incorrectly unreferenced/canceled watcher):{info}"));
        return Zv::undef();
    }
    match result {
        None => Zv::null(),
        Some(i) => run::take_interrupt(i).unwrap_or(Zv::undef()),
    }
}

unsafe extern "C" fn suspend(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let mut v = suspend_value(call::this(ex));
    if !zval::is_undef(v.as_ptr()) {
        zval::write(rv, v.take());
    }
}

/// The stack traces of the fibers suspended through this driver's suspensions.
unsafe fn pending_traces(d: D) -> String {
    let mut info = String::new();
    for p in d.c().suspensions.values() {
        let so = p as *mut sys::zend_object;
        let f = st(so).fiber;
        if f.is_null() || fiber::status(f) != fiber::STATUS_SUSPENDED {
            continue;
        }
        let trace = fiber_trace(f);
        info.push_str("\n\n");
        info.push_str(&format_trace(trace.0.value.arr));
    }
    info
}

/// ReflectionFiber::getTrace(DEBUG_BACKTRACE_IGNORE_ARGS) of a suspended fiber.
unsafe fn fiber_trace(f: *mut sys::zend_object) -> Zv {
    let zf = f as *mut sys::zend_fiber;
    let eg = zend::eg();
    let bottom = (*zf).stack_bottom;
    let prev = (*bottom).prev_execute_data;
    (*bottom).prev_execute_data = null_mut();
    let saved = (*eg).current_execute_data;
    (*eg).current_execute_data = (*zf).execute_data;
    let mut rv = Zv::undef();
    sys::zend_fetch_debug_backtrace(rv.as_ptr(), 0, 2, 0);
    (*eg).current_execute_data = saved;
    (*bottom).prev_execute_data = prev;
    rv
}

/// DriverSuspension::formatStacktrace() / TracingDriver::formatStacktrace(): "#i file:line class->fn()".
pub unsafe fn format_trace(trace: *mut sys::HashTable) -> String {
    let mut lines = Vec::new();
    let mut i = 0;
    hash::each(trace, |k, e| {
        let idx = match k {
            hash::Key::Index(n) => n.to_string(),
            hash::Key::Str(s) => zstr::str(s).into_owned(),
        };
        let _ = i;
        i += 1;
        let e = zval::deref(e);
        let mut line = format!("#{idx} ");
        if zval::ty(e) == zval::ARRAY {
            let a = (*e).value.arr;
            let get = |key: &[u8]| -> Option<String> {
                let z = hash::find_str(a, key);
                if z.is_null() {
                    return None;
                }
                let z = zval::deref(z);
                match zval::ty(z) {
                    zval::STRING => Some(zstr::str((*z).value.str_).into_owned()),
                    zval::LONG => Some((*z).value.lval.to_string()),
                    _ => None,
                }
            };
            if let (Some(f), Some(l)) = (get(b"file"), get(b"line")) {
                line.push_str(&format!("{f}:{l} "));
            }
            if let (Some(c), Some(t)) = (get(b"class"), get(b"type")) {
                line.push_str(&c);
                line.push_str(&t);
            }
            line.push_str(&get(b"function").unwrap_or_default());
            line.push_str("()");
        }
        lines.push(line);
        true
    });
    lines.join("\n")
}

unsafe extern "C" fn format_stacktrace(ex: Ex, rv: *mut sys::zval) {
    let mut t: *mut sys::zval = null_mut();
    if !zpp!(ex, "a", &mut t) {
        return;
    }
    let s = format_trace((*t).value.arr);
    zval::write(rv, zval::string(s.as_bytes()));
}
