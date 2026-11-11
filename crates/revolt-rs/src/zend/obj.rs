//! Objects carrying Rust state: `Native<T>` puts the state before the zend_object (whose
//! declared properties follow it), the handlers' `offset` finds it back. The state is dropped by
//! free_obj (once); objects whose free_obj ran are recognised with [`freed`] (the engine keeps
//! their memory until every free_obj of a GC run / of the shutdown is done, so references between
//! native objects may be checked that way, never followed into dropped state).

use super::{sys, zval};
use std::ffi::c_int;
use std::ptr::null_mut;

#[repr(C)]
pub struct Native<T> {
    pub state: T,
    pub std: sys::zend_object,
}

/// State types: how to report the zvals they hold to the cycle collector.
pub trait State: Sized {
    fn new(ce: *mut sys::zend_class_entry) -> Self;
    /// Calls `add` for each zval the state holds a reference to.
    fn gc(&mut self, add: &mut dyn FnMut(*mut sys::zval));
}

/// The state of an object created by [`create`] with handlers of the same `T`.
#[inline(always)]
pub unsafe fn state<'a, T>(o: *mut sys::zend_object) -> &'a mut T {
    let off = std::mem::offset_of!(Native<T>, std);
    &mut (*((o as *mut u8).sub(off) as *mut Native<T>)).state
}

/// Whether free_obj already ran for `o`.
#[inline(always)]
pub unsafe fn freed(o: *mut sys::zend_object) -> bool {
    (*o).gc.u.type_info & sys::IS_OBJ_FREE_CALLED != 0
}

/// Handlers for `T`: the standard ones plus the state's offset, free_obj, get_gc and a clone_obj
/// that refuses (native state cannot be duplicated safely; the class stays "cloneable" for
/// Reflection, as the PHP classes are).
pub unsafe fn handlers<T: State>() -> *mut sys::zend_object_handlers {
    let mut h: sys::zend_object_handlers = std::ptr::read(std::ptr::addr_of!(sys::std_object_handlers));
    // c_int before 8.6, size_t since (php-src 777a79cac9d)
    h.offset = std::mem::offset_of!(Native<T>, std) as _;
    h.free_obj = Some(free::<T>);
    h.get_gc = Some(get_gc::<T>);
    h.clone_obj = Some(no_clone::<T>);
    Box::leak(Box::new(h))
}

/// create_object for `T`'s classes (and the PHP classes extending them).
pub unsafe fn create<T: State>(ce: *mut sys::zend_class_entry, handlers: *const sys::zend_object_handlers) -> *mut sys::zend_object {
    let size = std::mem::size_of::<Native<T>>();
    let p = sys::ext_php_rs_zend_object_alloc(size, ce) as *mut Native<T>;
    std::ptr::write(std::ptr::addr_of_mut!((*p).state), T::new(ce));
    let o = std::ptr::addr_of_mut!((*p).std);
    sys::zend_object_std_init(o, ce);
    sys::object_properties_init(o, ce);
    (*o).handlers = handlers;
    o
}

unsafe extern "C" fn free<T: State>(o: *mut sys::zend_object) {
    std::ptr::drop_in_place(state::<T>(o) as *mut T);
    sys::zend_object_std_dtor(o);
}

unsafe extern "C" fn get_gc<T: State>(o: *mut sys::zend_object, table: *mut *mut sys::zval, n: *mut c_int) -> *mut sys::HashTable {
    let buf = sys::zend_get_gc_buffer_create();
    let mut add = |z: *mut sys::zval| {
        if zval::is_refcounted(z) {
            if (*buf).cur == (*buf).end {
                sys::zend_get_gc_buffer_grow(buf);
            }
            std::ptr::copy_nonoverlapping(z, (*buf).cur, 1);
            (*buf).cur = (*buf).cur.add(1);
        }
    };
    if !freed(o) {
        state::<T>(o).gc(&mut add);
    }
    let props = (*o).properties;
    if props.is_null() {
        // the declared properties (of PHP subclasses too)
        let base = std::ptr::addr_of_mut!((*o).properties_table) as *mut sys::zval;
        for i in 0..(*(*o).ce).default_properties_count as usize {
            add(base.add(i));
        }
    }
    *table = (*buf).start;
    *n = (*buf).cur.offset_from((*buf).start) as c_int;
    props
}

unsafe extern "C" fn no_clone<T: State>(o: *mut sys::zend_object) -> *mut sys::zend_object {
    let ce = (*o).ce;
    sys::zend_throw_error(null_mut(), c"Trying to clone an uncloneable object of class %s".as_ptr(), (*(*ce).name).val.as_ptr());
    // the VM releases the result of a failed clone: it must be an object
    create::<T>(ce, (*o).handlers)
}

/// OBJ_PROP_NUM(o, n)
#[inline(always)]
pub unsafe fn prop(o: *mut sys::zend_object, n: usize) -> *mut sys::zval {
    (std::ptr::addr_of_mut!((*o).properties_table) as *mut sys::zval).add(n)
}

/// Initialises/overwrites a declared property slot directly (bypassing readonly and visibility,
/// as the declaring class's own code may), releasing the old value.
#[inline]
pub unsafe fn set_prop(o: *mut sys::zend_object, n: usize, v: zval::Z) {
    let slot = prop(o, n);
    let mut old = zval::take(slot);
    zval::write(slot, v);
    // Z_PROP_FLAG_P: initialised, not re-initialisable
    (*slot).u2.extra = 0;
    zval::dtor(&mut old);
}
