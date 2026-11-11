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

// ---------------------------------------------------------------- values held across a call

/// Values the extension holds while a call made from a fiber runs (the callback, its arguments):
/// PHP keeps such values in the frames of the fiber, where the cycle collector sees them; ours
/// are reported by the fiber's get_gc, so a fiber suspended in a callback and only referenced
/// from a cycle is collected as Revolt's would be. Fibers made by [`new_tracked`] only.
pub mod inflight {
    use super::super::{sys, zval};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::c_int;
    use std::ptr::null_mut;

    thread_local! {
        static HELD: RefCell<HashMap<usize, Vec<zval::Z>>> = RefCell::new(HashMap::new());
    }

    static mut HANDLERS: *mut sys::zend_object_handlers = null_mut();
    static mut ORIG_GC: sys::zend_object_get_gc_t = None;
    static mut ORIG_FREE: sys::zend_object_free_obj_t = None;

    /// Our handlers for fiber objects (the engine's, with get_gc and free_obj wrapped).
    pub unsafe fn handlers(of: *mut sys::zend_object) -> *const sys::zend_object_handlers {
        if HANDLERS.is_null() {
            let mut h: sys::zend_object_handlers = std::ptr::read((*of).handlers);
            ORIG_GC = h.get_gc;
            ORIG_FREE = h.free_obj;
            h.get_gc = Some(get_gc);
            h.free_obj = Some(free_obj);
            HANDLERS = Box::leak(Box::new(h));
        }
        HANDLERS
    }

    /// Holds `values` for the running fiber `f` (owned) until [`take`].
    pub fn hold(f: *mut sys::zend_object, values: Vec<zval::Z>) {
        HELD.with(|h| h.borrow_mut().insert(f as usize, values));
    }

    /// The values held for `f` (owned by the caller again).
    pub fn take(f: *mut sys::zend_object) -> Vec<zval::Z> {
        HELD.with(|h| h.borrow_mut().remove(&(f as usize))).unwrap_or_default()
    }

    /// A pointer to the held values of `f` (stable until [`take`]): the parameters of the call.
    pub fn params(f: *mut sys::zend_object) -> (*mut zval::Z, usize) {
        HELD.with(|h| match h.borrow_mut().get_mut(&(f as usize)) {
            Some(v) => (v.as_mut_ptr(), v.len()),
            None => (null_mut(), 0),
        })
    }

    unsafe extern "C" fn get_gc(o: *mut sys::zend_object, table: *mut *mut sys::zval, n: *mut c_int) -> *mut sys::HashTable {
        let ht = ORIG_GC.unwrap()(o, table, n);
        let ptrs: Vec<*mut sys::zval> = HELD.with(|h| match h.borrow_mut().get_mut(&(o as usize)) {
            Some(v) => v.iter_mut().map(|z| &mut **z as *mut sys::zval).collect(),
            None => Vec::new(),
        });
        if ptrs.is_empty() {
            return ht;
        }
        let buf = std::ptr::addr_of_mut!((*super::super::eg()).get_gc_buffer);
        if *table != (*buf).start {
            // not the engine's buffer: leave the report as it is
            return ht;
        }
        (*buf).cur = (*buf).start.add(*n as usize);
        for z in ptrs {
            if zval::is_refcounted(z) {
                if (*buf).cur == (*buf).end {
                    sys::zend_get_gc_buffer_grow(buf);
                }
                std::ptr::copy_nonoverlapping(z, (*buf).cur, 1);
                (*buf).cur = (*buf).cur.add(1);
            }
        }
        *table = (*buf).start;
        *n = (*buf).cur.offset_from((*buf).start) as c_int;
        ht
    }

    unsafe extern "C" fn free_obj(o: *mut sys::zend_object) {
        ORIG_FREE.unwrap()(o);
        for mut z in take(o) {
            zval::dtor(&mut z);
        }
    }
}

/// new Fiber($callable) whose in-flight values are reported to the cycle collector.
pub unsafe fn new_tracked(callable: &mut Zv) -> *mut sys::zend_object {
    let f = new(callable);
    if !f.is_null() {
        (*f).handlers = inflight::handlers(f);
    }
    f
}
