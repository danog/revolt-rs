//! PHP fibers driven from Rust: Fiber objects whose function is a Closure over a static internal
//! function, started/resumed/suspended through zend_fiber_start/resume/suspend (PHP 8.4+, with
//! the checks the Fiber methods make first) or through the Fiber methods themselves (8.2/8.3).

use super::{call, sys, zstr, zval, zval::Zv};
use std::ptr::null_mut;

pub const STATUS_INIT: u32 = 0;
pub const STATUS_RUNNING: u32 = 1;
pub const STATUS_SUSPENDED: u32 = 2;
pub const STATUS_DEAD: u32 = 3;
/// ZEND_FIBER_FLAG_DESTROYED
const FLAG_DESTROYED: u32 = 1 << 2;

static mut FIBER_ERROR: *mut sys::zend_class_entry = null_mut();

/// MINIT: FiberError (its class entry is not exported).
pub unsafe fn minit() {
    FIBER_ERROR = call::lookup_class(b"fibererror");
}

pub fn fiber_error_ce() -> *mut sys::zend_class_entry {
    unsafe { FIBER_ERROR }
}

#[inline(always)]
unsafe fn zf(o: *mut sys::zend_object) -> *mut sys::zend_fiber {
    // zend_fiber starts with its zend_object
    o as *mut sys::zend_fiber
}

#[inline(always)]
pub unsafe fn status(o: *mut sys::zend_object) -> u32 {
    (*zf(o)).context.status as u32
}

/// Fiber::isTerminated()
#[inline(always)]
pub unsafe fn is_terminated(o: *mut sys::zend_object) -> bool {
    status(o) == STATUS_DEAD
}

/// Whether the fiber was destroyed while suspended (its stack unwound by the engine).
#[inline(always)]
pub unsafe fn is_destroyed(o: *mut sys::zend_object) -> bool {
    (*zf(o)).flags as u32 & FLAG_DESTROYED != 0
}

/// A static internal function usable as a Closure (for fiber bodies and callbacks): leaked.
pub unsafe fn internal_function(name: &str, handler: sys::zif_handler, scope: *mut sys::zend_class_entry) -> *mut sys::zend_function {
    let f: *mut sys::zend_function = Box::leak(Box::new(std::mem::zeroed::<sys::zend_function>()));
    let i = std::ptr::addr_of_mut!((*f).internal_function);
    (*i).type_ = sys::ZEND_INTERNAL_FUNCTION as u8;
    (*i).fn_flags = sys::ZEND_ACC_PUBLIC | super::class::STRICT;
    (*i).function_name = zstr::interned(name.as_bytes());
    (*i).scope = scope;
    (*i).handler = handler;
    f
}

/// A Closure over `func` with `$this` = `this` (a new reference) or none.
pub unsafe fn closure(func: *mut sys::zend_function, this: *mut sys::zend_object) -> Zv {
    let mut rv = Zv::undef();
    let mut t = if this.is_null() { zval::undef() } else { zval::obj_ref(this) };
    let scope = (*func).common.scope;
    sys::zend_create_fake_closure(rv.as_ptr(), func, scope, if this.is_null() { scope } else { (*this).ce }, if this.is_null() { null_mut() } else { &mut *t });
    zval::dtor(&mut t);
    rv
}

/// new Fiber($callable): the object (owned), or null with an exception.
pub unsafe fn new(callable: &mut Zv) -> *mut sys::zend_object {
    call::new_object(sys::zend_ce_fiber, std::slice::from_mut(&mut callable.0))
}

#[cfg(php84)]
unsafe fn blocked() -> bool {
    if sys::zend_fiber_switch_blocked() {
        sys::zend_throw_error(FIBER_ERROR, c"Cannot switch fibers in current execution context".as_ptr());
        return true;
    }
    false
}

/// $fiber->start(): the value of its first suspension (or its return), UNDEF on exception.
pub unsafe fn start(o: *mut sys::zend_object) -> Zv {
    #[cfg(php84)]
    {
        let mut rv = Zv::undef();
        if blocked() {
            return rv;
        }
        if status(o) != STATUS_INIT {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot start a fiber that has already been started".as_ptr());
            return rv;
        }
        let f = zf(o);
        (*f).fci.params = null_mut();
        (*f).fci.param_count = 0;
        (*f).fci.named_params = null_mut();
        if sys::zend_fiber_start(f, rv.as_ptr()) != sys::ZEND_RESULT_CODE_SUCCESS {
            return Zv::undef();
        }
        rv
    }
    #[cfg(not(php84))]
    call::method(o, b"start", &mut [])
}

/// $fiber->resume($value)
pub unsafe fn resume(o: *mut sys::zend_object, value: &mut zval::Z) -> Zv {
    #[cfg(php84)]
    {
        let mut rv = Zv::undef();
        if blocked() {
            return rv;
        }
        let f = zf(o);
        if status(o) != STATUS_SUSPENDED || !(*f).caller.is_null() {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot resume a fiber that is not suspended".as_ptr());
            return rv;
        }
        sys::zend_fiber_resume(f, &mut **value, rv.as_ptr());
        rv
    }
    #[cfg(not(php84))]
    call::method(o, b"resume", std::slice::from_mut(value))
}

/// $fiber->throw($exception)
pub unsafe fn throw_into(o: *mut sys::zend_object, ex: &mut zval::Z) -> Zv {
    call::method(o, b"throw", std::slice::from_mut(ex))
}

/// Fiber::suspend($value) from the running fiber: the value it is resumed with (UNDEF when an
/// exception was thrown into it, or when it could not suspend).
pub unsafe fn suspend(value: &mut zval::Z) -> Zv {
    #[cfg(php84)]
    {
        let mut rv = Zv::undef();
        let f = (*super::eg()).active_fiber;
        if f.is_null() {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot suspend outside of a fiber".as_ptr());
            return rv;
        }
        if (*f).flags as u32 & FLAG_DESTROYED != 0 {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot suspend in a force-closed fiber".as_ptr());
            return rv;
        }
        if blocked() {
            return rv;
        }
        sys::zend_fiber_suspend(f, &mut **value, rv.as_ptr());
        rv
    }
    #[cfg(not(php84))]
    {
        let suspend = call::find_method(sys::zend_ce_fiber, b"suspend");
        call::call(&call::Callee { func: suspend, obj: null_mut(), scope: sys::zend_ce_fiber }, std::slice::from_mut(value), null_mut())
    }
}
