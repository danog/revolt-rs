//! DriverFactory: always a NativeDriver (REVOLT_DRIVER is ignored: revolt-rs has no other
//! driver class, DIVERGENCES.md); REVOLT_DRIVER_DEBUG_TRACE wraps it in a TracingDriver.

use super::{ce, method as m, Ces, F_PRI as PRI, F_PUB as PUB, T_DRV};
use crate::zend::class::{self as k, c};
use crate::zend::{call, call::Ex, sys, zval};
use crate::zh;
use std::ptr::null_mut;

pub unsafe fn register(ces: &mut Ces) {
    ces.driver_factory = k::register_class(
        "Revolt\\EventLoop\\DriverFactory",
        vec![
            m("create", zh!(create_handler), PUB, T_DRV, vec![]),
            m("createDriverFromEnv", zh!(from_env_handler), PRI, c("Revolt\\EventLoop\\Driver", true), vec![]),
        ],
        null_mut(),
        k::FINAL,
    );
}

/// PHP's truthiness of an environment value.
fn truthy(v: &Option<String>) -> bool {
    matches!(v, Some(s) if !s.is_empty() && s != "0")
}

/// DriverFactory::create(): a new driver (owned), or null with an exception.
pub unsafe fn create() -> *mut sys::zend_object {
    let d = create_loop_driver();
    if d.is_null() {
        return d;
    }
    if truthy(&crate::zend::getenv("REVOLT_DRIVER_DEBUG_TRACE")) {
        let mut a = zval::obj_owned(d);
        let t = call::new_object(ce().tracing, std::slice::from_mut(&mut a));
        zval::dtor(&mut a);
        return t;
    }
    d
}

/// The event loop's own driver: a new NativeDriver (owned; null with an exception).
pub unsafe fn create_loop_driver() -> *mut sys::zend_object {
    call::new_object(ce().native, &mut [])
}

unsafe extern "C" fn create_handler(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let d = create();
    if !d.is_null() {
        zval::write(rv, zval::obj_owned(d));
    }
}

/// createDriverFromEnv(): null (REVOLT_DRIVER selects nothing).
unsafe extern "C" fn from_env_handler(ex: Ex, rv: *mut sys::zval) {
    if call::none(ex) {
        zval::write(rv, zval::null());
    }
}
