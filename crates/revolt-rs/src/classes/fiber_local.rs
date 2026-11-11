//! FiberLocal: per-fiber values, in WeakMaps as Revolt keeps them (fiber => (FiberLocal =>
//! [value])), `{main}` keyed by a dummy Fiber. The maps are held per thread and released at
//! RSHUTDOWN.

use super::{arg as a, ce, method as m, Ces, F_PRI as PRI, F_PUB as PUB, F_STA as STA};
use super::{T_CL, T_MIX, T_V};
use crate::zend::class::{self as k, c, Default as Dv, NONE};
use crate::zend::{self, call, call::Ex, fiber, hash, obj, sys, zval, zval::Zv};
use crate::{zh, zpp};
use std::cell::Cell;
use std::ptr::null_mut;

thread_local! {
    /// FiberLocal::$localStorage (a WeakMap, owned) or null
    static STORAGE: Cell<*mut sys::zend_object> = const { Cell::new(null_mut()) };
    /// FiberLocal::$mainFiber (owned) or null
    static MAIN_FIBER: Cell<*mut sys::zend_object> = const { Cell::new(null_mut()) };
}

static mut INITIALIZER: usize = 0;
static mut WEAKMAP: *mut sys::zend_class_entry = null_mut();
static mut NOOP_FN: *mut sys::zend_function = null_mut();

pub unsafe fn register(ces: &mut Ces) {
    let cls = k::register_class(
        "Revolt\\EventLoop\\FiberLocal",
        vec![
            m("clear", zh!(clear_handler), PUB | STA, T_V, vec![]),
            m("getFiberStorage", zh!(get_fiber_storage), PRI | STA, c("WeakMap", false), vec![]),
            m("__construct", zh!(construct), PUB, NONE, vec![a("initializer", T_CL)]),
            m("set", zh!(set), PUB, T_V, vec![a("value", T_MIX)]),
            m("unset", zh!(unset), PUB, T_V, vec![]),
            m("get", zh!(get), PUB, T_MIX, vec![]),
        ],
        null_mut(),
        k::FINAL,
    );
    k::prop(cls, "mainFiber", c("Fiber", true), k::PRIVATE | k::STATIC, Dv::Null);
    k::prop(cls, "localStorage", c("WeakMap", true), k::PRIVATE | k::STATIC, Dv::Null);
    k::prop(cls, "initializer", T_CL, k::PRIVATE | k::READONLY, Dv::Undef);
    INITIALIZER = k::prop_slot(cls, "initializer");
    WEAKMAP = call::lookup_class(b"weakmap");
    NOOP_FN = fiber::internal_function("{closure}", zh!(noop), cls);
    ces.fiber_local = cls;
}

unsafe extern "C" fn noop(_ex: Ex, _rv: *mut sys::zval) {}

/// RSHUTDOWN: the maps and the dummy fiber go.
pub unsafe fn rshutdown() {
    for cell in [&STORAGE, &MAIN_FIBER] {
        let o = cell.with(|c| c.replace(null_mut()));
        if !o.is_null() {
            let mut z = zval::obj_owned(o);
            zval::dtor(&mut z);
        }
    }
}

unsafe fn new_weakmap() -> *mut sys::zend_object {
    let mut z = zval::undef();
    sys::object_init_ex(&mut *z, WEAKMAP);
    z.value.obj
}

/// $map[$key] (borrowed) or null
unsafe fn wm_get(map: *mut sys::zend_object, key: *mut sys::zend_object) -> *mut sys::zval {
    let mut k = zval::obj_ref(key);
    let r = if ((*(*map).handlers).has_dimension.unwrap())(map, &mut *k, 0) != 0 {
        let mut rv = zval::undef();
        ((*(*map).handlers).read_dimension.unwrap())(map, &mut *k, 0, &mut *rv)
    } else {
        null_mut()
    };
    zval::dtor(&mut k);
    r
}

unsafe fn wm_set(map: *mut sys::zend_object, key: *mut sys::zend_object, v: zval::Z) {
    let mut k = zval::obj_ref(key);
    let mut v = v;
    ((*(*map).handlers).write_dimension.unwrap())(map, &mut *k, &mut *v);
    zval::dtor(&mut v);
    zval::dtor(&mut k);
}

unsafe fn wm_unset(map: *mut sys::zend_object, key: *mut sys::zend_object) {
    let mut k = zval::obj_ref(key);
    ((*(*map).handlers).unset_dimension.unwrap())(map, &mut *k);
    zval::dtor(&mut k);
}

/// FiberLocal::clear(): the current fiber's values go (after each event loop callback).
#[inline]
pub fn clear() {
    let storage = STORAGE.with(|c| c.get());
    if storage.is_null() {
        return;
    }
    unsafe {
        let mut f = zend::active_fiber();
        if f.is_null() {
            f = MAIN_FIBER.with(|c| c.get());
        }
        if f.is_null() {
            return;
        }
        (*storage).gc.refcount += 1;
        wm_unset(storage, f);
        let mut z = zval::obj_owned(storage);
        zval::dtor(&mut z);
    }
}

/// getFiberStorage(): the current fiber's WeakMap (a new reference), or null on failure.
unsafe fn fiber_storage() -> *mut sys::zend_object {
    let mut f = zend::active_fiber();
    if f.is_null() {
        f = MAIN_FIBER.with(|c| c.get());
        if f.is_null() {
            let mut cl = fiber::closure(NOOP_FN, null_mut());
            f = fiber::new(&mut cl);
            if f.is_null() {
                return null_mut();
            }
            MAIN_FIBER.with(|c| c.set(f));
        }
    }
    let mut storage = STORAGE.with(|c| c.get());
    if storage.is_null() {
        storage = new_weakmap();
        STORAGE.with(|c| c.set(storage));
    }
    let z = wm_get(storage, f);
    if !z.is_null() && zval::ty(z) == zval::OBJECT {
        let o = (*z).value.obj;
        (*o).gc.refcount += 1;
        return o;
    }
    let inner = new_weakmap();
    (*inner).gc.refcount += 1;
    wm_set(storage, f, zval::obj_owned(inner));
    inner
}

unsafe extern "C" fn clear_handler(ex: Ex, _rv: *mut sys::zval) {
    if call::none(ex) {
        clear();
    }
}

unsafe extern "C" fn get_fiber_storage(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let s = fiber_storage();
    if !s.is_null() {
        zval::write(rv, zval::obj_owned(s));
    }
}

unsafe extern "C" fn construct(ex: Ex, _rv: *mut sys::zval) {
    let mut cl: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut cl, sys::zend_ce_closure) {
        return;
    }
    let o = call::this(ex);
    if !zval::is_undef(obj::prop(o, INITIALIZER)) {
        call::throw_error("Cannot modify readonly property Revolt\\EventLoop\\FiberLocal::$initializer");
        return;
    }
    obj::set_prop(o, INITIALIZER, zval::copy(cl));
}

unsafe extern "C" fn set(ex: Ex, _rv: *mut sys::zval) {
    let mut v: *mut sys::zval = null_mut();
    if !zpp!(ex, "z", &mut v) {
        return;
    }
    let s = fiber_storage();
    if s.is_null() {
        return;
    }
    let arr = hash::new(1);
    hash::push(arr, zval::copy_deref(v));
    wm_set(s, call::this(ex), zval::arr_owned(arr));
    drop(Zv(zval::obj_owned(s)));
}

unsafe extern "C" fn unset(ex: Ex, _rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let s = fiber_storage();
    if s.is_null() {
        return;
    }
    wm_unset(s, call::this(ex));
    drop(Zv(zval::obj_owned(s)));
}

unsafe extern "C" fn get(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let this = call::this(ex);
    let s = fiber_storage();
    if s.is_null() {
        return;
    }
    let holder = Zv(zval::obj_owned(s));
    if wm_get(s, this).is_null() {
        let init = obj::prop(this, INITIALIZER);
        let mut v = call::call_closure(zval::obj(init), &mut []);
        if call::has_exception() {
            return;
        }
        let arr = hash::new(1);
        hash::push(arr, v.take());
        wm_set(s, this, zval::arr_owned(arr));
    }
    let z = wm_get(s, this);
    if !z.is_null() && zval::ty(z) == zval::ARRAY {
        let first = hash::find_index((*z).value.arr, 0);
        if !first.is_null() {
            zval::write(rv, zval::copy_deref(first));
        }
    }
    drop(holder);
    let _ = ce();
}
