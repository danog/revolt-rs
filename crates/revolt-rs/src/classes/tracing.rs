//! Driver\TracingDriver: wraps a Driver and records the creation and cancellation stack traces of
//! callbacks (for dump() and for InvalidCallbackError's info).

use super::{arg as a, ce, method as m, Ces, F_PRI as PRI, F_PUB as PUB};
use super::{T_A, T_DRV, T_S};
use crate::zend::class::{self as k, Default as Dv, NONE};
use crate::zend::{call, call::Ex, fiber, hash, obj, sys, zstr, zval, zval::Zv};
use crate::{zh, zpp};
use std::ptr::null_mut;

pub struct TracingState {
    driver: Zv,
    /// id => true (insertion order matters for dump())
    enabled: Zv,
    unreferenced: Zv,
    creation: Zv,
    cancel: Zv,
    /// defer()/delay() wrappers: id => the user's closure
    wrapped: Zv,
}

impl obj::State for TracingState {
    fn new(_ce: *mut sys::zend_class_entry) -> Self {
        unsafe {
            TracingState {
                driver: Zv::undef(),
                enabled: Zv(zval::empty_array()),
                unreferenced: Zv(zval::empty_array()),
                creation: Zv(zval::empty_array()),
                cancel: Zv(zval::empty_array()),
                wrapped: Zv(zval::empty_array()),
            }
        }
    }
    fn gc(&mut self, add: &mut dyn FnMut(*mut sys::zval)) {
        add(&mut *self.driver.0);
        add(&mut *self.wrapped.0);
    }
}

static mut HANDLERS: *mut sys::zend_object_handlers = null_mut();
static mut WRAPPER_FN: *mut sys::zend_function = null_mut();

unsafe extern "C" fn create_object(cls: *mut sys::zend_class_entry) -> *mut sys::zend_object {
    obj::create::<TracingState>(cls, HANDLERS)
}

#[inline(always)]
unsafe fn st<'a>(o: *mut sys::zend_object) -> &'a mut TracingState {
    obj::state::<TracingState>(o)
}

pub unsafe fn register(ces: &mut Ces) {
    HANDLERS = obj::handlers::<TracingState>();
    let mut ms = super::driver_method_list(PUB, &|n| handler(n));
    ms.extend(vec![
        m("__construct", zh!(construct), PUB, NONE, vec![a("driver", T_DRV)]),
        m("dump", zh!(dump), PUB, T_S, vec![]),
        m("getCreationTrace", zh!(get_creation_trace), PRI, T_S, vec![a("callbackId", T_S)]),
        m("getCancelTrace", zh!(get_cancel_trace), PRI, T_S, vec![a("callbackId", T_S)]),
        m("formatStacktrace", zh!(format_stacktrace), PRI, T_S, vec![a("trace", T_A)]),
    ]);
    let cls = k::register_class("Revolt\\EventLoop\\Driver\\TracingDriver", ms, null_mut(), k::FINAL | k::NOT_SERIALIZABLE);
    k::implements(cls, ces.driver);
    (*cls).__bindgen_anon_2.create_object = Some(create_object);
    k::prop(cls, "driver", T_DRV, k::PRIVATE | k::READONLY, Dv::Undef);
    for n in ["enabledCallbacks", "unreferencedCallbacks", "creationTraces", "cancelTraces"] {
        k::prop(cls, n, T_A, k::PRIVATE, Dv::EmptyArray);
    }
    WRAPPER_FN = fiber::internal_function("{closure}", zh!(wrapper), cls);
    ces.tracing = cls;
}

fn handler(name: &'static str) -> k::Handler {
    match name {
        "defer" => zh!(defer),
        "delay" => zh!(delay),
        "repeat" | "onReadable" | "onWritable" | "onSignal" => zh!(create_traced),
        "enable" => zh!(enable),
        "cancel" => zh!(cancel),
        "disable" => zh!(disable),
        "reference" => zh!(reference),
        "unreference" => zh!(unreference),
        _ => zh!(pass),
    }
}

/// The (separated) array of a state field.
unsafe fn arr(z: &mut Zv) -> *mut sys::HashTable {
    let a = z.0.value.arr;
    if (*a).gc.refcount > 1 || (*a).gc.u.type_info & sys::GC_IMMUTABLE != 0 {
        z.set(zval::arr_owned(sys::zend_array_dup(a)));
    }
    z.0.value.arr
}

/// The name of the executing function, lower case (the method being forwarded).
unsafe fn fname(ex: Ex) -> Vec<u8> {
    zstr::bytes((*(*ex).func).common.function_name).to_ascii_lowercase()
}

/// $this->driver->method(...$args): the result (UNDEF with an exception).
unsafe fn inner(ex: Ex, lc: &[u8], args: &mut [zval::Z]) -> Zv {
    let d = st(call::this(ex)).driver.obj();
    if d.is_null() {
        call::throw_error("Typed property Revolt\\EventLoop\\Driver\\TracingDriver::$driver must not be accessed before initialization");
        return Zv::undef();
    }
    (*d).gc.refcount += 1;
    let r = call::method(d, lc, args);
    let mut z = zval::obj_owned(d);
    zval::dtor(&mut z);
    r
}

unsafe fn frame_args(ex: Ex) -> Vec<zval::Z> {
    (0..call::num_args(ex) as usize).map(|i| zval::copy_deref(call::arg(ex, i))).collect()
}

unsafe fn drop_args(mut v: Vec<zval::Z>) {
    for a in v.iter_mut() {
        zval::dtor(a);
    }
}

/// formatStacktrace(debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS)) from the current frame.
unsafe fn trace_here() -> Vec<u8> {
    let mut rv = Zv::undef();
    sys::zend_fetch_debug_backtrace(rv.as_ptr(), 0, 2, 0);
    if zval::ty(&rv.0) != zval::ARRAY {
        return Vec::new();
    }
    super::suspension::format_trace(rv.0.value.arr).into_bytes()
}

unsafe fn record_creation(o: *mut sys::zend_object, id: *mut sys::zend_string) {
    let s = st(o);
    let t = trace_here();
    hash::set_zstr(arr(&mut s.creation), id, zval::string(&t));
    hash::set_zstr(arr(&mut s.enabled), id, zval::bool(true));
}

/// Methods forwarded unchanged.
unsafe extern "C" fn pass(ex: Ex, rv: *mut sys::zval) {
    let args = frame_args(ex);
    let mut args = args;
    let mut r = inner(ex, &fname(ex), &mut args);
    drop_args(args);
    if !zval::is_undef(r.as_ptr()) {
        zval::write(rv, r.take());
    }
}

/// repeat/onReadable/onWritable/onSignal: forwarded, the creation trace recorded.
unsafe extern "C" fn create_traced(ex: Ex, rv: *mut sys::zval) {
    let mut args = frame_args(ex);
    let mut r = inner(ex, &fname(ex), &mut args);
    drop_args(args);
    if zval::ty(r.as_ptr()) == zval::STRING {
        record_creation(call::this(ex), r.0.value.str_);
    }
    if !zval::is_undef(r.as_ptr()) {
        zval::write(rv, r.take());
    }
}

/// defer/delay: the closure is wrapped so that the callback is cancelled (traced) when it runs.
unsafe fn wrapped_create(ex: Ex, rv: *mut sys::zval, lc: &[u8], closure_arg: usize) {
    let o = call::this(ex);
    let mut args = frame_args(ex);
    let user = Zv(zval::copy(&args[closure_arg]));
    let wrapper = fiber::closure(WRAPPER_FN, o);
    zval::dtor(&mut args[closure_arg]);
    args[closure_arg] = zval::copy(&wrapper.0);
    let mut r = inner(ex, lc, &mut args);
    drop_args(args);
    if zval::ty(r.as_ptr()) == zval::STRING {
        let id = r.0.value.str_;
        hash::set_zstr(arr(&mut st(o).wrapped), id, zval::copy(&user.0));
        record_creation(o, id);
    }
    if !zval::is_undef(r.as_ptr()) {
        zval::write(rv, r.take());
    }
}

unsafe extern "C" fn defer(ex: Ex, rv: *mut sys::zval) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut z, sys::zend_ce_closure) {
        return;
    }
    wrapped_create(ex, rv, b"defer", 0)
}

unsafe extern "C" fn delay(ex: Ex, rv: *mut sys::zval) {
    let mut d = 0f64;
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "dO", &mut d, &mut z, sys::zend_ce_closure) {
        return;
    }
    wrapped_create(ex, rv, b"delay", 1)
}

/// The wrapper: `function (...$args) use ($closure) { $this->cancel($args[0]); return $closure(...$args); }`
unsafe extern "C" fn wrapper(ex: Ex, rv: *mut sys::zval) {
    let o = call::this(ex);
    let mut args = frame_args(ex);
    if args.is_empty() || zval::ty(&args[0]) != zval::STRING {
        drop_args(args);
        return;
    }
    let id = args[0].value.str_;
    let w = arr(&mut st(o).wrapped);
    let z = sys::zend_hash_find(w, id);
    if z.is_null() {
        drop_args(args);
        return;
    }
    let user = Zv(zval::copy(z));
    sys::zend_hash_del(w, id);
    cancel_traced(o, id);
    if call::has_exception() {
        drop_args(args);
        return;
    }
    let mut r = call::call_closure(user.obj(), &mut args);
    drop_args(args);
    if !zval::is_undef(r.as_ptr()) {
        zval::write(rv, r.take());
    }
}

/// enable()/reference(): InvalidCallbackError gets the traces added.
unsafe fn with_info(ex: Ex, rv: *mut sys::zval, lc: &[u8], on_ok: impl FnOnce(&mut TracingState, *mut sys::zend_string)) {
    let mut s: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "S", &mut s) {
        return;
    }
    let o = call::this(ex);
    let mut a = zval::str_ref(s);
    let r = inner(ex, lc, std::slice::from_mut(&mut a));
    zval::dtor(&mut a);
    drop(r);
    if call::has_exception() {
        let e = (*crate::zend::eg()).exception;
        if call::instance_of((*e).ce, ce().invalid_callback_error) {
            let (c, k) = traces(o, s);
            super::errors::add_info(e, b"Creation trace", &c);
            super::errors::add_info(e, b"Cancellation trace", &k);
        }
        return;
    }
    on_ok(st(o), s);
    zval::write(rv, zval::str_ref(s));
}

unsafe fn traces(o: *mut sys::zend_object, id: *mut sys::zend_string) -> (Vec<u8>, Vec<u8>) {
    let s = st(o);
    let get = |z: &Zv, default: &[u8]| -> Vec<u8> {
        let v = sys::zend_hash_find(z.0.value.arr, id);
        if v.is_null() || zval::ty(v) != zval::STRING {
            default.to_vec()
        } else {
            zstr::bytes((*v).value.str_).to_vec()
        }
    };
    (get(&s.creation, b"No creation trace, yet."), get(&s.cancel, b"No cancellation trace, yet."))
}

unsafe extern "C" fn enable(ex: Ex, rv: *mut sys::zval) {
    with_info(ex, rv, b"enable", |s, id| {
        hash::set_zstr(arr(&mut s.enabled), id, zval::bool(true));
    })
}

unsafe extern "C" fn reference(ex: Ex, rv: *mut sys::zval) {
    with_info(ex, rv, b"reference", |s, id| {
        sys::zend_hash_del(arr(&mut s.unreferenced), id);
    })
}

/// cancel() of the inner driver, the cancellation trace recorded (once).
unsafe fn cancel_traced(o: *mut sys::zend_object, id: *mut sys::zend_string) {
    let d = st(o).driver.obj();
    if d.is_null() {
        return;
    }
    let mut a = zval::str_ref(id);
    (*d).gc.refcount += 1;
    drop(call::method(d, b"cancel", std::slice::from_mut(&mut a)));
    let mut z = zval::obj_owned(d);
    zval::dtor(&mut z);
    zval::dtor(&mut a);
    if call::has_exception() {
        return;
    }
    let s = st(o);
    if sys::zend_hash_find(s.cancel.0.value.arr, id).is_null() {
        let t = trace_here();
        hash::set_zstr(arr(&mut s.cancel), id, zval::string(&t));
    }
    sys::zend_hash_del(arr(&mut s.enabled), id);
    sys::zend_hash_del(arr(&mut s.unreferenced), id);
}

unsafe extern "C" fn cancel(ex: Ex, _rv: *mut sys::zval) {
    let mut s: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "S", &mut s) {
        return;
    }
    cancel_traced(call::this(ex), s);
}

unsafe extern "C" fn disable(ex: Ex, rv: *mut sys::zval) {
    let mut s: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "S", &mut s) {
        return;
    }
    let mut a = zval::str_ref(s);
    drop(inner(ex, b"disable", std::slice::from_mut(&mut a)));
    zval::dtor(&mut a);
    if call::has_exception() {
        return;
    }
    sys::zend_hash_del(arr(&mut st(call::this(ex)).enabled), s);
    zval::write(rv, zval::str_ref(s));
}

unsafe extern "C" fn unreference(ex: Ex, rv: *mut sys::zval) {
    let mut s: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "S", &mut s) {
        return;
    }
    let mut a = zval::str_ref(s);
    drop(inner(ex, b"unreference", std::slice::from_mut(&mut a)));
    zval::dtor(&mut a);
    if call::has_exception() {
        return;
    }
    hash::set_zstr(arr(&mut st(call::this(ex)).unreferenced), s, zval::bool(true));
    zval::write(rv, zval::str_ref(s));
}

unsafe extern "C" fn construct(ex: Ex, _rv: *mut sys::zval) {
    let mut z: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut z, ce().driver) {
        return;
    }
    let s = st(call::this(ex));
    if !zval::is_undef(&s.driver.0) {
        call::throw_error("Cannot modify readonly property Revolt\\EventLoop\\Driver\\TracingDriver::$driver");
        return;
    }
    s.driver.set(zval::copy(z));
}

unsafe extern "C" fn dump(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    let o = call::this(ex);
    let s = st(o);
    let mut out = b"Enabled, referenced callbacks keeping the loop running: ".to_vec();
    let unref = s.unreferenced.0.value.arr;
    let mut ids = Vec::new();
    hash::each(s.enabled.0.value.arr, |k, _| {
        if let hash::Key::Str(id) = k {
            if sys::zend_hash_find(unref, id).is_null() {
                ids.push(id);
            }
        }
        true
    });
    for id in ids {
        out.extend_from_slice(b"Callback identifier: ");
        out.extend_from_slice(zstr::bytes(id));
        out.extend_from_slice(b"\r\n");
        out.extend(traces(o, id).0);
        out.extend_from_slice(b"\r\n\r\n");
    }
    while out.last().is_some_and(|c| c.is_ascii_whitespace() || *c == 0 || *c == 0x0b) {
        out.pop();
    }
    zval::write(rv, zval::string(&out));
}

unsafe extern "C" fn get_creation_trace(ex: Ex, rv: *mut sys::zval) {
    let mut s: *mut sys::zend_string = null_mut();
    if zpp!(ex, "S", &mut s) {
        zval::write(rv, zval::string(&traces(call::this(ex), s).0));
    }
}

unsafe extern "C" fn get_cancel_trace(ex: Ex, rv: *mut sys::zval) {
    let mut s: *mut sys::zend_string = null_mut();
    if zpp!(ex, "S", &mut s) {
        zval::write(rv, zval::string(&traces(call::this(ex), s).1));
    }
}

unsafe extern "C" fn format_stacktrace(ex: Ex, rv: *mut sys::zval) {
    let mut t: *mut sys::zval = null_mut();
    if zpp!(ex, "a", &mut t) {
        let s = super::suspension::format_trace((*t).value.arr);
        zval::write(rv, zval::string(s.as_bytes()));
    }
}
