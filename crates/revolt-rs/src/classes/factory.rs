//! DriverFactory: REVOLT_DRIVER, else UvDriver, EvDriver, EventDriver when supported, else
//! StreamSelectDriver; REVOLT_DRIVER_DEBUG_TRACE wraps it in a TracingDriver.

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

/// createDriverFromEnv(): Some(null) when REVOLT_DRIVER is unset; None with an exception.
unsafe fn from_env() -> Option<*mut sys::zend_object> {
    let v = crate::zend::getenv("REVOLT_DRIVER");
    if !truthy(&v) {
        return Some(null_mut());
    }
    let name = v.unwrap();
    let cls = call::lookup_class_autoload(name.as_bytes());
    if call::has_exception() {
        return None;
    }
    if cls.is_null() {
        call::throw_error(&format!("Driver '{name}' does not exist."));
        return None;
    }
    let drv = ce().driver;
    if cls == drv || !call::instance_of(cls, drv) {
        call::throw_error(&format!("Driver '{name}' is not a subclass of 'Revolt\\EventLoop\\Driver'."));
        return None;
    }
    let o = call::new_object(cls, &mut []);
    if o.is_null() {
        None
    } else {
        Some(o)
    }
}

/// DriverFactory::create(): a new driver (owned), or null with an exception.
pub unsafe fn create() -> *mut sys::zend_object {
    let Some(mut d) = from_env() else { return null_mut() };
    if d.is_null() {
        let ces = ce();
        let cls = if super::driver::extension_loaded(b"uv") {
            ces.uv
        } else if super::driver::extension_loaded(b"ev") {
            ces.ev
        } else if super::driver::extension_loaded(b"event") {
            ces.event
        } else {
            ces.stream_select
        };
        d = call::new_object(cls, &mut []);
        if d.is_null() {
            return d;
        }
    }
    if truthy(&crate::zend::getenv("REVOLT_DRIVER_DEBUG_TRACE")) {
        let mut a = zval::obj_owned(d);
        let t = call::new_object(ce().tracing, std::slice::from_mut(&mut a));
        zval::dtor(&mut a);
        return t;
    }
    d
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

unsafe extern "C" fn from_env_handler(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    if let Some(d) = from_env() {
        zval::write(rv, if d.is_null() { zval::null() } else { zval::obj_owned(d) });
    }
}
