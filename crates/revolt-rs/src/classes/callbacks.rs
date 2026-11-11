//! Internal\DriverCallback and its subclasses (plain objects with declared properties), the
//! DriverCallback "views" of records handed to PHP drivers, and Internal\TimerQueue (Revolt's
//! heap, on the same private arrays).

use super::{arg as a, arg_default as ad, method as m, Ces, F_PRI as PRI, F_PUB as PUB};
use super::{T_A, T_B, T_CL, T_F, T_I, T_MIX, T_NEVER, T_S, T_V};
use crate::core::run::D;
use crate::core::{Cb, Kind, ViewProp};
use crate::zend::class::{self as k, c, Default as Dv, NONE};
use crate::zend::{call, call::Ex, hash, obj, sys, zstr, zval};
use crate::{zh, zpp};
use std::ptr::null_mut;

struct Slots {
    invokable: usize,
    enabled: usize,
    referenced: usize,
    id: usize,
    closure: usize,
    interval: usize,
    expiration: usize,
    repeat: usize,
    stream: usize,
    signal: usize,
    tq_callbacks: usize,
    tq_pointers: usize,
}

static mut SLOTS: Slots = Slots { invokable: 0, enabled: 0, referenced: 0, id: 0, closure: 0, interval: 0, expiration: 0, repeat: 0, stream: 0, signal: 0, tq_callbacks: 0, tq_pointers: 0 };

fn slots() -> &'static Slots {
    unsafe { &*std::ptr::addr_of!(SLOTS) }
}

pub unsafe fn register(ces: &mut Ces) {
    let ro = k::PUBLIC | k::READONLY;
    let dc = k::register_class(
        "Revolt\\EventLoop\\Internal\\DriverCallback",
        vec![
            m("__construct", zh!(dc_construct), PUB, NONE, vec![a("id", T_S), a("closure", T_CL)]),
            m("__get", zh!(magic_get), PUB, T_NEVER, vec![a("property", T_S)]),
            m("__set", zh!(magic_set), PUB, T_NEVER, vec![a("property", T_S), a("value", T_MIX)]),
        ],
        null_mut(),
        k::ABSTRACT,
    );
    k::prop(dc, "invokable", T_B, k::PUBLIC, Dv::Bool(false));
    k::prop(dc, "enabled", T_B, k::PUBLIC, Dv::Bool(true));
    k::prop(dc, "referenced", T_B, k::PUBLIC, Dv::Bool(true));
    k::prop(dc, "id", T_S, ro, Dv::Undef);
    k::prop(dc, "closure", T_CL, ro, Dv::Undef);
    ces.driver_callback = dc;
    ces.defer_callback = k::register_class("Revolt\\EventLoop\\Internal\\DeferCallback", vec![], dc, k::FINAL);
    ces.timer_callback = k::register_class(
        "Revolt\\EventLoop\\Internal\\TimerCallback",
        vec![m("__construct", zh!(timer_construct), PUB, NONE, vec![a("id", T_S), a("interval", T_F), a("callback", T_CL), a("expiration", T_F), ad("repeat", T_B, "false")])],
        dc,
        k::FINAL,
    );
    k::prop(ces.timer_callback, "interval", T_F, ro, Dv::Undef);
    k::prop(ces.timer_callback, "expiration", T_F, k::PUBLIC, Dv::Undef);
    k::prop(ces.timer_callback, "repeat", T_B, ro, Dv::Undef);
    ces.stream_callback = k::register_class(
        "Revolt\\EventLoop\\Internal\\StreamCallback",
        vec![m("__construct", zh!(stream_construct), PUB, NONE, vec![a("id", T_S), a("closure", T_CL), a("stream", T_MIX)])],
        dc,
        k::ABSTRACT,
    );
    k::prop(ces.stream_callback, "stream", T_MIX, ro, Dv::Undef);
    ces.readable_callback = k::register_class("Revolt\\EventLoop\\Internal\\StreamReadableCallback", vec![], ces.stream_callback, k::FINAL);
    ces.writable_callback = k::register_class("Revolt\\EventLoop\\Internal\\StreamWritableCallback", vec![], ces.stream_callback, k::FINAL);
    ces.signal_callback = k::register_class(
        "Revolt\\EventLoop\\Internal\\SignalCallback",
        vec![m("__construct", zh!(signal_construct), PUB, NONE, vec![a("id", T_S), a("closure", T_CL), a("signal", T_I)])],
        dc,
        k::FINAL,
    );
    k::prop(ces.signal_callback, "signal", T_I, ro, Dv::Undef);

    let tc = c("Revolt\\EventLoop\\Internal\\TimerCallback", false);
    ces.timer_queue = k::register_class(
        "Revolt\\EventLoop\\Internal\\TimerQueue",
        vec![
            m("insert", zh!(tq_insert), PUB, T_V, vec![a("callback", tc)]),
            m("remove", zh!(tq_remove), PUB, T_V, vec![a("callback", tc)]),
            m("extract", zh!(tq_extract), PUB, c("Revolt\\EventLoop\\Internal\\TimerCallback", true), vec![a("now", T_F)]),
            m("peek", zh!(tq_peek), PUB, crate::zend::class::t(k::T_FLOAT | k::T_NULL), vec![]),
            m("heapifyUp", zh!(tq_heapify_up), PRI, T_V, vec![a("node", T_I)]),
            m("heapifyDown", zh!(tq_heapify_down), PRI, T_V, vec![a("node", T_I)]),
            m("swap", zh!(tq_swap), PRI, T_V, vec![a("left", T_I), a("right", T_I)]),
            m("removeAndRebuild", zh!(tq_remove_and_rebuild), PRI, T_V, vec![a("node", T_I)]),
        ],
        null_mut(),
        k::FINAL,
    );
    k::prop(ces.timer_queue, "callbacks", T_A, k::PRIVATE, Dv::EmptyArray);
    k::prop(ces.timer_queue, "pointers", T_A, k::PRIVATE, Dv::EmptyArray);

    let s = &mut *std::ptr::addr_of_mut!(SLOTS);
    s.invokable = k::prop_slot(dc, "invokable");
    s.enabled = k::prop_slot(dc, "enabled");
    s.referenced = k::prop_slot(dc, "referenced");
    s.id = k::prop_slot(dc, "id");
    s.closure = k::prop_slot(dc, "closure");
    s.interval = k::prop_slot(ces.timer_callback, "interval");
    s.expiration = k::prop_slot(ces.timer_callback, "expiration");
    s.repeat = k::prop_slot(ces.timer_callback, "repeat");
    s.stream = k::prop_slot(ces.stream_callback, "stream");
    s.signal = k::prop_slot(ces.signal_callback, "signal");
    s.tq_callbacks = k::prop_slot(ces.timer_queue, "callbacks");
    s.tq_pointers = k::prop_slot(ces.timer_queue, "pointers");
}

/// Initialises a readonly property from its class's own constructor; false (Error thrown) when
/// it is initialised already.
unsafe fn init_ro(o: *mut sys::zend_object, slot: usize, name: &str, v: zval::Z) -> bool {
    if !zval::is_undef(obj::prop(o, slot)) {
        let mut v = v;
        zval::dtor(&mut v);
        call::throw_error(&format!("Cannot modify readonly property {}::${name}", String::from_utf8_lossy(call::class_name((*o).ce))));
        return false;
    }
    obj::set_prop(o, slot, v);
    true
}

unsafe extern "C" fn dc_construct(ex: Ex, _rv: *mut sys::zval) {
    let mut id: *mut sys::zend_string = null_mut();
    let mut cl: *mut sys::zval = null_mut();
    if !zpp!(ex, "SO", &mut id, &mut cl, sys::zend_ce_closure) {
        return;
    }
    let o = call::this(ex);
    let s = slots();
    let _ = init_ro(o, s.id, "id", zval::str_ref(id)) && init_ro(o, s.closure, "closure", zval::copy(cl));
}

unsafe extern "C" fn timer_construct(ex: Ex, _rv: *mut sys::zval) {
    let mut id: *mut sys::zend_string = null_mut();
    let mut interval = 0f64;
    let mut cl: *mut sys::zval = null_mut();
    let mut exp = 0f64;
    let mut repeat = false;
    if !zpp!(ex, "SdOd|b", &mut id, &mut interval, &mut cl, sys::zend_ce_closure, &mut exp, &mut repeat) {
        return;
    }
    let o = call::this(ex);
    let s = slots();
    // promoted: interval, expiration, repeat, then the parent's id and closure
    if !init_ro(o, s.interval, "interval", zval::double(interval)) {
        return;
    }
    obj::set_prop(o, s.expiration, zval::double(exp));
    let _ = init_ro(o, s.repeat, "repeat", zval::bool(repeat)) && init_ro(o, s.id, "id", zval::str_ref(id)) && init_ro(o, s.closure, "closure", zval::copy(cl));
}

unsafe extern "C" fn stream_construct(ex: Ex, _rv: *mut sys::zval) {
    let mut id: *mut sys::zend_string = null_mut();
    let mut cl: *mut sys::zval = null_mut();
    let mut st: *mut sys::zval = null_mut();
    if !zpp!(ex, "SOz", &mut id, &mut cl, sys::zend_ce_closure, &mut st) {
        return;
    }
    let o = call::this(ex);
    let s = slots();
    let _ = init_ro(o, s.stream, "stream", zval::copy_deref(st)) && init_ro(o, s.id, "id", zval::str_ref(id)) && init_ro(o, s.closure, "closure", zval::copy(cl));
}

unsafe extern "C" fn signal_construct(ex: Ex, _rv: *mut sys::zval) {
    let mut id: *mut sys::zend_string = null_mut();
    let mut cl: *mut sys::zval = null_mut();
    let mut sig: i64 = 0;
    if !zpp!(ex, "SOl", &mut id, &mut cl, sys::zend_ce_closure, &mut sig) {
        return;
    }
    let o = call::this(ex);
    let s = slots();
    let _ = init_ro(o, s.signal, "signal", zval::long(sig)) && init_ro(o, s.id, "id", zval::str_ref(id)) && init_ro(o, s.closure, "closure", zval::copy(cl));
}

unsafe extern "C" fn magic_get(ex: Ex, _rv: *mut sys::zval) {
    let mut p: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "S", &mut p) {
        return;
    }
    call::throw_error(&format!("Unknown property '{}'", zstr::str(p)));
}

unsafe extern "C" fn magic_set(ex: Ex, _rv: *mut sys::zval) {
    let mut p: *mut sys::zend_string = null_mut();
    let mut v: *mut sys::zval = null_mut();
    if !zpp!(ex, "Sz", &mut p, &mut v) {
        return;
    }
    call::throw_error(&format!("Unknown property '{}'", zstr::str(p)));
}

// ---------------------------------------------------------------- views of records

/// The DriverCallback object of a record (created on first use, kept by the record).
pub unsafe fn view(d: D, id: u64) -> *mut sys::zend_object {
    let Some(r) = d.c().record(id) else { return null_mut() };
    if !r.view.is_null() {
        return r.view;
    }
    let ces = super::ce();
    let cls = match r.kind {
        Kind::Defer => ces.defer_callback,
        Kind::Delay | Kind::Repeat => ces.timer_callback,
        Kind::Readable => ces.readable_callback,
        Kind::Writable => ces.writable_callback,
        Kind::Signal => ces.signal_callback,
    };
    let mut z = zval::undef();
    if sys::object_init_ex(&mut *z, cls) != sys::ZEND_RESULT_CODE_SUCCESS {
        return null_mut();
    }
    let o = z.value.obj;
    let s = slots();
    obj::set_prop(o, s.invokable, zval::bool(r.invokable));
    obj::set_prop(o, s.enabled, zval::bool(r.enabled));
    obj::set_prop(o, s.referenced, zval::bool(r.referenced));
    obj::set_prop(o, s.id, zval::str_ref(r.id_str));
    if let Cb::Php { closure, .. } = &r.cb {
        obj::set_prop(o, s.closure, zval::copy(&closure.0));
    }
    match r.kind {
        Kind::Delay | Kind::Repeat => {
            obj::set_prop(o, s.interval, zval::double(r.interval));
            obj::set_prop(o, s.expiration, zval::double(r.expiration));
            obj::set_prop(o, s.repeat, zval::bool(r.kind == Kind::Repeat));
        }
        Kind::Readable | Kind::Writable => obj::set_prop(o, s.stream, zval::copy(&r.stream.0)),
        Kind::Signal => obj::set_prop(o, s.signal, zval::long(r.signal as i64)),
        Kind::Defer => {}
    }
    r.view = o;
    o
}

/// Writes a record change through to its view.
pub unsafe fn view_write(o: *mut sys::zend_object, p: ViewProp, v: zval::Z) {
    let s = slots();
    let slot = match p {
        ViewProp::Invokable => s.invokable,
        ViewProp::Enabled => s.enabled,
        ViewProp::Referenced => s.referenced,
        ViewProp::Expiration => s.expiration,
    };
    obj::set_prop(o, slot, v);
}

/// The callback number of a DriverCallback object (0: none).
pub unsafe fn view_id(o: *mut sys::zend_object) -> u64 {
    let z = obj::prop(o, slots().id);
    if zval::ty(z) != zval::STRING {
        return 0;
    }
    crate::core::string_to_id(zstr::bytes((*z).value.str_)).unwrap_or(0)
}

// ---------------------------------------------------------------- TimerQueue

/// Revolt's TimerQueue, on its own `$callbacks` (a list of TimerCallback) and `$pointers`
/// (id => position) arrays.
struct Tq(*mut sys::zend_object);

impl Tq {
    /// The array of a property, separated for writing.
    unsafe fn arr(&self, slot: usize) -> *mut sys::HashTable {
        let z = obj::prop(self.0, slot);
        if zval::ty(z) != zval::ARRAY {
            obj::set_prop(self.0, slot, zval::empty_array());
        }
        let a = (*z).value.arr;
        if (*a).gc.refcount > 1 || (*a).gc.u.type_info & sys::GC_IMMUTABLE != 0 {
            let dup = sys::zend_array_dup(a);
            obj::set_prop(self.0, slot, zval::arr_owned(dup));
        }
        (*obj::prop(self.0, slot)).value.arr
    }

    unsafe fn cbs(&self) -> *mut sys::HashTable {
        self.arr(slots().tq_callbacks)
    }

    unsafe fn ptrs(&self) -> *mut sys::HashTable {
        self.arr(slots().tq_pointers)
    }

    unsafe fn count(&self) -> i64 {
        hash::count(self.cbs()) as i64
    }

    unsafe fn get(&self, n: i64) -> *mut sys::zend_object {
        let z = hash::find_index(self.cbs(), n as u64);
        if z.is_null() {
            null_mut()
        } else {
            zval::obj(zval::deref(z))
        }
    }

    unsafe fn set(&self, n: i64, o: *mut sys::zend_object) {
        hash::set_index(self.cbs(), n as u64, zval::obj_ref(o));
    }

    unsafe fn exp(o: *mut sys::zend_object) -> f64 {
        let z = obj::prop(o, slots().expiration);
        match zval::ty(z) {
            zval::DOUBLE => (*z).value.dval,
            zval::LONG => (*z).value.lval as f64,
            _ => 0.0,
        }
    }

    unsafe fn id(o: *mut sys::zend_object) -> *mut sys::zend_string {
        let z = obj::prop(o, slots().id);
        if zval::ty(z) == zval::STRING {
            (*z).value.str_
        } else {
            sys::zend_empty_string
        }
    }

    unsafe fn set_ptr(&self, o: *mut sys::zend_object, n: i64) {
        hash::set_zstr(self.ptrs(), Self::id(o), zval::long(n));
    }

    unsafe fn ptr(&self, o: *mut sys::zend_object) -> Option<i64> {
        let s = Self::id(o);
        let z = sys::zend_hash_find(self.ptrs(), s);
        if z.is_null() || zval::ty(z) != zval::LONG {
            None
        } else {
            Some((*z).value.lval)
        }
    }

    unsafe fn insert(&self, cb: *mut sys::zend_object) {
        let node = self.count();
        self.set(node, cb);
        self.set_ptr(cb, node);
        self.up(node);
    }

    unsafe fn up(&self, mut node: i64) {
        let entry = self.get(node);
        if entry.is_null() {
            return;
        }
        while node != 0 {
            let parent = (node - 1) >> 1;
            let p = self.get(parent);
            if p.is_null() || !(Self::exp(entry) < Self::exp(p)) {
                break;
            }
            self.swap(node, parent);
            node = parent;
        }
    }

    unsafe fn down(&self, mut node: i64) {
        let length = self.count();
        loop {
            let child = (node << 1) + 1;
            if child >= length {
                break;
            }
            let (cn, cc) = (self.get(node), self.get(child));
            let cr = if child + 1 < length { self.get(child + 1) } else { null_mut() };
            let swap = if Self::exp(cc) < Self::exp(cn) && (child + 1 >= length || Self::exp(cc) < Self::exp(cr)) {
                child
            } else if child + 1 < length && Self::exp(cr) < Self::exp(cn) {
                child + 1
            } else {
                break;
            };
            self.swap(node, swap);
            node = swap;
        }
    }

    unsafe fn swap(&self, left: i64, right: i64) {
        let temp = self.get(left);
        let r = self.get(right);
        if temp.is_null() || r.is_null() {
            return;
        }
        (*temp).gc.refcount += 1;
        self.set(left, r);
        self.set_ptr(r, left);
        self.set(right, temp);
        self.set_ptr(temp, right);
        let mut z = zval::obj_owned(temp);
        zval::dtor(&mut z);
    }

    unsafe fn remove_and_rebuild(&self, node: i64) {
        let length = self.count() - 1;
        let at = self.get(node);
        let last = self.get(length);
        if at.is_null() || last.is_null() {
            return;
        }
        let id = Self::id(at);
        zstr::addref(id);
        self.set(node, last);
        self.set_ptr(last, node);
        sys::zend_hash_index_del(self.cbs(), length as u64);
        sys::zend_hash_del(self.ptrs(), id);
        zstr::release(id);
        if node < length {
            let parent = (node - 1) >> 1;
            if parent >= 0 && Self::exp(self.get(node)) < Self::exp(self.get(parent)) {
                self.up(node);
            } else {
                self.down(node);
            }
        }
    }
}

unsafe fn timer_arg(ex: Ex) -> Option<*mut sys::zend_object> {
    let mut z: *mut sys::zval = null_mut();
    if zpp!(ex, "O", &mut z, super::ce().timer_callback) {
        Some((*z).value.obj)
    } else {
        None
    }
}

unsafe fn int_arg(ex: Ex) -> Option<i64> {
    let mut n: i64 = 0;
    if zpp!(ex, "l", &mut n) {
        Some(n)
    } else {
        None
    }
}

unsafe extern "C" fn tq_insert(ex: Ex, _rv: *mut sys::zval) {
    if let Some(cb) = timer_arg(ex) {
        Tq(call::this(ex)).insert(cb);
    }
}

unsafe extern "C" fn tq_remove(ex: Ex, _rv: *mut sys::zval) {
    if let Some(cb) = timer_arg(ex) {
        let q = Tq(call::this(ex));
        if let Some(n) = q.ptr(cb) {
            q.remove_and_rebuild(n);
        }
    }
}

unsafe extern "C" fn tq_extract(ex: Ex, rv: *mut sys::zval) {
    let mut now = 0f64;
    if !zpp!(ex, "d", &mut now) {
        return;
    }
    let q = Tq(call::this(ex));
    if q.count() == 0 {
        zval::write(rv, zval::null());
        return;
    }
    let cb = q.get(0);
    if cb.is_null() || Tq::exp(cb) > now {
        zval::write(rv, zval::null());
        return;
    }
    (*cb).gc.refcount += 1;
    q.remove_and_rebuild(0);
    zval::write(rv, zval::obj_owned(cb));
}

unsafe extern "C" fn tq_peek(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let cb = Tq(call::this(ex)).get(0);
    zval::write(rv, if cb.is_null() { zval::null() } else { zval::double(Tq::exp(cb)) });
}

unsafe extern "C" fn tq_heapify_up(ex: Ex, _rv: *mut sys::zval) {
    if let Some(n) = int_arg(ex) {
        Tq(call::this(ex)).up(n);
    }
}

unsafe extern "C" fn tq_heapify_down(ex: Ex, _rv: *mut sys::zval) {
    if let Some(n) = int_arg(ex) {
        Tq(call::this(ex)).down(n);
    }
}

unsafe extern "C" fn tq_swap(ex: Ex, _rv: *mut sys::zval) {
    let (mut l, mut r) = (0i64, 0i64);
    if zpp!(ex, "ll", &mut l, &mut r) {
        Tq(call::this(ex)).swap(l, r);
    }
}

unsafe extern "C" fn tq_remove_and_rebuild(ex: Ex, _rv: *mut sys::zval) {
    if let Some(n) = int_arg(ex) {
        Tq(call::this(ex)).remove_and_rebuild(n);
    }
}
