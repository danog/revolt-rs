//! Revolt\EventLoop: the static facade over the driver of the calling thread. With a native
//! driver (not overridden in PHP) the operations run directly on its core; any other Driver
//! gets the same call through PHP.

use super::driver::{self as dr, bit, native_for};
use super::{arg as a, av, ce, method as m, Ces, F_PRI as PRI, F_PUB as PUB, F_STA as STA};
use super::{T_A, T_B, T_CBT, T_CL, T_CLN, T_DRV, T_F, T_I, T_MIX, T_S, T_SUSP, T_V};
use crate::core::run::D;
use crate::zend::class::{self as k, Default as Dv, NONE};
use crate::zend::{call, call::Ex, sys, zstr, zval};
use crate::{zh, zpp};
use std::cell::Cell;
use std::ptr::null_mut;

thread_local! {
    /// EventLoop::$driver (owned) or null
    static DRIVER: Cell<*mut sys::zend_object> = const { Cell::new(null_mut()) };
}

/// The current driver without creating one (borrowed), or null.
#[inline(always)]
pub fn current() -> *mut sys::zend_object {
    DRIVER.with(|c| c.get())
}

/// EventLoop::getDriver() (borrowed), or null with an exception. Always a NativeDriver:
/// setDriver() is refused, so this is the loop's only driver.
#[inline(always)]
pub fn driver() -> *mut sys::zend_object {
    let d = current();
    if !d.is_null() {
        return d;
    }
    unsafe {
        let n = super::factory::create_loop_driver();
        if n.is_null() {
            return n;
        }
        // EventLoop::$driver too (Reflection shows it, as the PHP library's)
        store(n);
        let name = zstr::new(b"driver");
        let mut v = zval::obj_owned(n);
        sys::zend_update_static_property_ex(ce().event_loop, name, &mut *v);
        zval::dtor(&mut v);
        zstr::release(name);
    }
    current()
}

/// Replaces the thread's driver (a new reference is taken).
unsafe fn store(d: *mut sys::zend_object) {
    if !d.is_null() {
        (*d).gc.refcount += 1;
    }
    let old = DRIVER.with(|c| c.replace(d));
    if !old.is_null() {
        let mut z = zval::obj_owned(old);
        zval::dtor(&mut z);
    }
}

/// EventLoop::setDriver()'s message: the loop's driver cannot be replaced.
pub const SET_DRIVER_REFUSED: &str = "Revolt\\EventLoop::setDriver() is not supported: the event loop always runs on its native driver";

/// RSHUTDOWN: the driver goes.
pub unsafe fn rshutdown() {
    store(null_mut());
}

pub unsafe fn register(ces: &mut Ces) {
    let ms = vec![
        m("setDriver", zh!(set_driver_h), PUB | STA, T_V, vec![a("driver", T_DRV)]),
        m("queue", zh!(queue), PUB | STA, T_V, vec![a("closure", T_CL), av("args", T_MIX)]),
        m("defer", zh!(defer), PUB | STA, T_S, vec![a("closure", T_CL)]),
        m("delay", zh!(delay), PUB | STA, T_S, vec![a("delay", T_F), a("closure", T_CL)]),
        m("repeat", zh!(repeat), PUB | STA, T_S, vec![a("interval", T_F), a("closure", T_CL)]),
        m("onReadable", zh!(on_readable), PUB | STA, T_S, vec![a("stream", T_MIX), a("closure", T_CL)]),
        m("onWritable", zh!(on_writable), PUB | STA, T_S, vec![a("stream", T_MIX), a("closure", T_CL)]),
        m("onSignal", zh!(on_signal), PUB | STA, T_S, vec![a("signal", T_I), a("closure", T_CL)]),
        m("enable", zh!(enable), PUB | STA, T_S, vec![a("callbackId", T_S)]),
        m("disable", zh!(disable), PUB | STA, T_S, vec![a("callbackId", T_S)]),
        m("cancel", zh!(cancel), PUB | STA, T_V, vec![a("callbackId", T_S)]),
        m("reference", zh!(reference), PUB | STA, T_S, vec![a("callbackId", T_S)]),
        m("unreference", zh!(unreference), PUB | STA, T_S, vec![a("callbackId", T_S)]),
        m("setErrorHandler", zh!(set_error_handler), PUB | STA, T_V, vec![a("errorHandler", T_CLN)]),
        m("getErrorHandler", zh!(get_error_handler), PUB | STA, T_CLN, vec![]),
        m("getIdentifiers", zh!(get_identifiers), PUB | STA, T_A, vec![]),
        m("getType", zh!(get_type), PUB | STA, T_CBT, vec![a("callbackId", T_S)]),
        m("isEnabled", zh!(is_enabled), PUB | STA, T_B, vec![a("callbackId", T_S)]),
        m("isReferenced", zh!(is_referenced), PUB | STA, T_B, vec![a("callbackId", T_S)]),
        m("getDriver", zh!(get_driver), PUB | STA, T_DRV, vec![]),
        m("getSuspension", zh!(get_suspension), PUB | STA, T_SUSP, vec![]),
        m("run", zh!(run), PUB | STA, T_V, vec![]),
        m("__construct", zh!(dr::private_noop), PRI, NONE, vec![]),
    ];
    let cls = k::register_class("Revolt\\EventLoop", ms, null_mut(), k::FINAL);
    k::prop(cls, "driver", T_DRV, k::PRIVATE | k::STATIC, Dv::Undef);
    ces.event_loop = cls;
}

/// The argument shapes of the facade, validated before a call goes to a PHP driver (so the
/// errors name EventLoop's method, as Revolt's do).
#[derive(Clone, Copy)]
enum Shape {
    None,
    Closure,
    ClosureVariadic,
    FloatClosure,
    MixedClosure,
    IntClosure,
    Id,
    NullableClosure,
}

unsafe fn validate(ex: Ex, shape: Shape) -> bool {
    let mut z: *mut sys::zval = null_mut();
    let mut z2: *mut sys::zval = null_mut();
    let mut f = 0f64;
    let mut l: i64 = 0;
    let mut s: *mut sys::zend_string = null_mut();
    let mut rest: *mut sys::zval = null_mut();
    let mut n: u32 = 0;
    let mut named: *mut sys::HashTable = null_mut();
    let cl = sys::zend_ce_closure;
    match shape {
        Shape::None => call::none(ex),
        Shape::Closure => zpp!(ex, "O", &mut z, cl),
        Shape::ClosureVariadic => zpp!(ex, "O*", &mut z, cl, &mut rest, &mut n, &mut named),
        Shape::FloatClosure => zpp!(ex, "dO", &mut f, &mut z, cl),
        Shape::MixedClosure => zpp!(ex, "zO", &mut z2, &mut z, cl),
        Shape::IntClosure => zpp!(ex, "lO", &mut l, &mut z, cl),
        Shape::Id => zpp!(ex, "S", &mut s),
        Shape::NullableClosure => zpp!(ex, "O!", &mut z, cl),
    }
}

/// $driver->method(...) with the frame's arguments (named ones included).
unsafe fn forward(ex: Ex, rv: *mut sys::zval, drv: *mut sys::zend_object, lc: &[u8], shape: Shape) {
    if !validate(ex, shape) {
        return;
    }
    let n = call::num_args(ex) as usize;
    let mut args: Vec<zval::Z> = (0..n).map(|i| zval::copy_deref(call::arg(ex, i))).collect();
    let f = call::find_method((*drv).ce, lc);
    let mut r = if f.is_null() {
        call::method(drv, lc, &mut args)
    } else {
        (*drv).gc.refcount += 1;
        let r = call::call(&call::Callee { func: f, obj: drv, scope: (*drv).ce }, &mut args, call::extra_named(ex));
        let mut z = zval::obj_owned(drv);
        zval::dtor(&mut z);
        r
    };
    for a in args.iter_mut() {
        zval::dtor(a);
    }
    if !zval::is_undef(r.as_ptr()) {
        zval::write(rv, r.take());
    }
}

macro_rules! facade {
    ($name:ident, $lc:literal, $imp:path, $shape:expr) => {
        unsafe extern "C" fn $name(ex: Ex, rv: *mut sys::zval) {
            let drv = driver();
            if drv.is_null() {
                return;
            }
            if native_for(drv, bit($lc)) {
                // the driver stays alive through the call (setDriver() may drop it)
                (*drv).gc.refcount += 1;
                $imp(ex, rv, D(drv));
                let mut z = zval::obj_owned(drv);
                zval::dtor(&mut z);
            } else {
                forward(ex, rv, drv, $lc.as_bytes(), $shape);
            }
        }
    };
}

facade!(queue, "queue", dr::queue_d, Shape::ClosureVariadic);
facade!(defer, "defer", dr::defer_d, Shape::Closure);
facade!(delay, "delay", dr::delay_d, Shape::FloatClosure);
facade!(repeat, "repeat", dr::repeat_d, Shape::FloatClosure);
facade!(on_readable, "onreadable", dr::on_readable_d, Shape::MixedClosure);
facade!(on_writable, "onwritable", dr::on_writable_d, Shape::MixedClosure);
facade!(on_signal, "onsignal", dr::on_signal_d, Shape::IntClosure);
facade!(enable, "enable", dr::enable_d, Shape::Id);
facade!(disable, "disable", dr::disable_d, Shape::Id);
facade!(cancel, "cancel", dr::cancel_d, Shape::Id);
facade!(reference, "reference", dr::reference_d, Shape::Id);
facade!(unreference, "unreference", dr::unreference_d, Shape::Id);
facade!(set_error_handler, "seterrorhandler", dr::set_error_handler_d, Shape::NullableClosure);
facade!(get_error_handler, "geterrorhandler", dr::get_error_handler_d, Shape::None);
facade!(get_identifiers, "getidentifiers", dr::get_identifiers_d, Shape::None);
facade!(get_type, "gettype", dr::get_type_d, Shape::Id);
facade!(is_enabled, "isenabled", dr::is_enabled_d, Shape::Id);
facade!(is_referenced, "isreferenced", dr::is_referenced_d, Shape::Id);
facade!(get_suspension, "getsuspension", dr::get_suspension_d, Shape::None);
facade!(run, "run", dr::run_d, Shape::None);

unsafe extern "C" fn get_driver(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let d = driver();
    if !d.is_null() {
        zval::write(rv, zval::obj_ref(d));
    }
}

unsafe extern "C" fn set_driver_h(ex: Ex, _rv: *mut sys::zval) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut z, ce().driver) {
        return;
    }
    call::throw(ce().unsupported_feature, SET_DRIVER_REFUSED);
}
