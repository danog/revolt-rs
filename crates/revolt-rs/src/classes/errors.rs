//! InvalidCallbackError, UncaughtThrowable (Error subclasses with private constructors and
//! static factories) and Internal\ClosureHelper::getDescription().

use super::ce;
use crate::core::Desc;
use crate::zend::{call, call::Ex, class, hash, obj, sys, zstr, zval, zval::Zv};
use crate::zpp;
use std::ptr::null_mut;

struct Slots {
    raw_message: usize,
    callback_id: usize,
    info: usize,
    message: usize,
}

static mut SLOTS: Slots = Slots { raw_message: 0, callback_id: 0, info: 0, message: 0 };

fn slots() -> &'static mut Slots {
    unsafe { &mut *std::ptr::addr_of_mut!(SLOTS) }
}

/// MINIT, after registration.
pub unsafe fn minit() {
    let c = ce().invalid_callback_error;
    let s = slots();
    s.raw_message = class::prop_slot(c, "rawMessage");
    s.callback_id = class::prop_slot(c, "callbackId");
    s.info = class::prop_slot(c, "info");
    s.message = class::prop_slot(sys::zend_ce_error, "message");
}

/// Error::__construct($message, $code, $previous) on a new object of `cls`.
unsafe fn new_error(cls: *mut sys::zend_class_entry, message: &[u8], code: i64, previous: *mut sys::zend_object) -> *mut sys::zend_object {
    let mut z = zval::undef();
    if sys::object_init_ex(&mut *z, cls) != sys::ZEND_RESULT_CODE_SUCCESS {
        return null_mut();
    }
    let o = z.value.obj;
    let mut args = [zval::string(message), zval::long(code), if previous.is_null() { zval::null() } else { zval::obj_ref(previous) }];
    let ctor = (*sys::zend_ce_error).constructor;
    let _ = call::call(&call::Callee { func: ctor, obj: o, scope: sys::zend_ce_error }, &mut args, null_mut());
    for a in args.iter_mut() {
        zval::dtor(a);
    }
    o
}

/// new InvalidCallbackError($callbackId, $code, $message)
unsafe fn invalid_callback(id: *mut sys::zend_string, code: i64, message: &[u8]) -> *mut sys::zend_object {
    let o = new_error(ce().invalid_callback_error, message, code, null_mut());
    if o.is_null() {
        return o;
    }
    let s = slots();
    obj::set_prop(o, s.callback_id, zval::str_ref(id));
    obj::set_prop(o, s.raw_message, zval::string(message));
    o
}

/// InvalidCallbackError::invalidIdentifier($id), thrown (the id reference is taken over).
pub unsafe fn throw_invalid_identifier(id: *mut sys::zend_string) {
    let mut msg = b"Invalid callback identifier ".to_vec();
    msg.extend_from_slice(zstr::bytes(id));
    let o = invalid_callback(id, 2, &msg);
    zstr::release(id);
    if !o.is_null() {
        call::rethrow(o);
    }
}

/// InvalidCallbackError::nonNullReturn($id, $closure) (not thrown).
pub unsafe fn non_null_return(id: u64, closure: *mut sys::zend_object) -> *mut sys::zend_object {
    let s = crate::core::id_to_string(id);
    let ids = zstr::new(&s);
    let mut msg = b"Non-null return value received from callback ".to_vec();
    msg.extend_from_slice(description(closure).as_bytes());
    let o = invalid_callback(ids, 1, &msg);
    zstr::release(ids);
    o
}

pub unsafe extern "C" fn icr_invalid_identifier(ex: Ex, rv: *mut sys::zval) {
    let mut id: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "S", &mut id) {
        return;
    }
    let mut msg = b"Invalid callback identifier ".to_vec();
    msg.extend_from_slice(zstr::bytes(id));
    let o = invalid_callback(id, 2, &msg);
    if !o.is_null() {
        zval::write(rv, zval::obj_owned(o));
    }
}

pub unsafe extern "C" fn icr_non_null_return(ex: Ex, rv: *mut sys::zval) {
    let mut id: *mut sys::zend_string = null_mut();
    let mut cl: *mut sys::zval = null_mut();
    if !zpp!(ex, "SO", &mut id, &mut cl, sys::zend_ce_closure) {
        return;
    }
    let mut msg = b"Non-null return value received from callback ".to_vec();
    msg.extend_from_slice(description((*cl).value.obj).as_bytes());
    let o = invalid_callback(id, 1, &msg);
    if !o.is_null() {
        zval::write(rv, zval::obj_owned(o));
    }
}

/// The private constructor (reachable through Reflection only).
pub unsafe extern "C" fn icr_construct(ex: Ex, _rv: *mut sys::zval) {
    let mut id: *mut sys::zend_string = null_mut();
    let mut code: i64 = 0;
    let mut message: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "SlS", &mut id, &mut code, &mut message) {
        return;
    }
    let o = call::this(ex);
    let mut args = [zval::str_ref(message), zval::long(code)];
    let ctor = (*sys::zend_ce_error).constructor;
    let _ = call::call(&call::Callee { func: ctor, obj: o, scope: sys::zend_ce_error }, &mut args, null_mut());
    for a in args.iter_mut() {
        zval::dtor(a);
    }
    let s = slots();
    obj::set_prop(o, s.callback_id, zval::str_ref(id));
    obj::set_prop(o, s.raw_message, zval::str_ref(message));
}

pub unsafe extern "C" fn icr_get_callback_id(ex: Ex, rv: *mut sys::zval) {
    if !call::none(ex) {
        return;
    }
    zval::write(rv, zval::copy(obj::prop(call::this(ex), slots().callback_id)));
}

pub unsafe extern "C" fn icr_add_info(ex: Ex, _rv: *mut sys::zval) {
    let mut key: *mut sys::zend_string = null_mut();
    let mut message: *mut sys::zend_string = null_mut();
    if !zpp!(ex, "SS", &mut key, &mut message) {
        return;
    }
    add_info(call::this(ex), zstr::bytes(key), zstr::bytes(message));
}

/// InvalidCallbackError::addInfo()
pub unsafe fn add_info(o: *mut sys::zend_object, key: &[u8], message: &[u8]) {
    let s = slots();
    let z = obj::prop(o, s.info);
    let info = if zval::ty(z) == zval::ARRAY { sys::zend_array_dup((*z).value.arr) } else { hash::new(1) };
    hash::set_str(info, key, zval::string(message));
    let mut text = Vec::new();
    let raw = obj::prop(o, s.raw_message);
    if zval::ty(raw) == zval::STRING {
        text.extend_from_slice(zstr::bytes((*raw).value.str_));
    }
    hash::each(info, |k, v| {
        text.extend_from_slice(b"\r\n\r\n");
        match k {
            hash::Key::Str(s) => text.extend_from_slice(zstr::bytes(s)),
            hash::Key::Index(i) => text.extend_from_slice(i.to_string().as_bytes()),
        }
        text.extend_from_slice(b": ");
        let v = zval::deref(v);
        if zval::ty(v) == zval::STRING {
            text.extend_from_slice(zstr::bytes((*v).value.str_));
        }
        true
    });
    obj::set_prop(o, s.info, zval::arr_owned(info));
    obj::set_prop(o, s.message, zval::string(&text));
}

// ---------------------------------------------------------------- UncaughtThrowable

const CALLBACK_FMT: (&str, &str) = ("Uncaught ", " thrown in event loop callback ");
const HANDLER_FMT: (&str, &str) = ("Uncaught ", " thrown in event loop error handler ");

/// The message of `previous`, through getMessage().
unsafe fn message_of(previous: *mut sys::zend_object) -> Vec<u8> {
    let mut r = call::method(previous, b"getmessage", &mut []);
    let z = zval::deref(r.as_ptr());
    if zval::ty(z) == zval::STRING {
        zstr::bytes((*z).value.str_).to_vec()
    } else {
        Vec::new()
    }
}

unsafe fn uncaught_message(handler: bool, desc: &str, previous: *mut sys::zend_object) -> Vec<u8> {
    let (a, b) = if handler { HANDLER_FMT } else { CALLBACK_FMT };
    let mut m = a.as_bytes().to_vec();
    // the NUL of an anonymous class name shows as '@'
    m.extend(call::class_name((*previous).ce).iter().map(|&c| if c == 0 { b'@' } else { c }));
    m.extend_from_slice(b.as_bytes());
    m.extend_from_slice(desc.as_bytes());
    let pm = message_of(previous);
    if !pm.is_empty() {
        m.extend_from_slice(b": ");
        m.extend_from_slice(&pm);
    }
    if !handler {
        // "%s; use Revolt\EventLoop::setErrorHandler() ..." comes before the message suffix
        let head_len = a.len() + call::class_name((*previous).ce).len() + b.len() + desc.len();
        let suffix = m.split_off(head_len);
        m.extend_from_slice(b"; use Revolt\\EventLoop::setErrorHandler() to gracefully handle such exceptions");
        m.extend_from_slice(&suffix);
    }
    m
}

/// UncaughtThrowable::throwingCallback() / throwingErrorHandler() (not thrown).
pub unsafe fn uncaught(desc: &Desc, previous: *mut sys::zend_object, handler: bool) -> *mut sys::zend_object {
    let d = match desc {
        Desc::Closure(c) => description(c.obj()),
        Desc::Static(s) => s.to_string(),
    };
    let msg = uncaught_message(handler, &d, previous);
    new_error(ce().uncaught_throwable, &msg, 0, previous)
}

unsafe fn ut_factory(ex: Ex, rv: *mut sys::zval, handler: bool) {
    let mut cl: *mut sys::zval = null_mut();
    let mut prev: *mut sys::zval = null_mut();
    if !zpp!(ex, "OO", &mut cl, sys::zend_ce_closure, &mut prev, sys::zend_ce_throwable) {
        return;
    }
    let o = uncaught(&Desc::Closure(Zv(zval::copy(cl))), (*prev).value.obj, handler);
    if !o.is_null() {
        zval::write(rv, zval::obj_owned(o));
    }
}

pub unsafe extern "C" fn ut_throwing_callback(ex: Ex, rv: *mut sys::zval) {
    ut_factory(ex, rv, false)
}

pub unsafe extern "C" fn ut_throwing_error_handler(ex: Ex, rv: *mut sys::zval) {
    ut_factory(ex, rv, true)
}

/// The private constructor: sprintf($message, class, description, ": message").
pub unsafe extern "C" fn ut_construct(ex: Ex, _rv: *mut sys::zval) {
    let mut fmt: *mut sys::zend_string = null_mut();
    let mut cl: *mut sys::zval = null_mut();
    let mut prev: *mut sys::zval = null_mut();
    if !zpp!(ex, "SOO", &mut fmt, &mut cl, sys::zend_ce_closure, &mut prev, sys::zend_ce_throwable) {
        return;
    }
    let p = (*prev).value.obj;
    let class: Vec<u8> = call::class_name((*p).ce).iter().map(|&c| if c == 0 { b'@' } else { c }).collect();
    let desc = description((*cl).value.obj);
    let pm = message_of(p);
    let suffix = if pm.is_empty() { Vec::new() } else { [b": ".as_slice(), &pm].concat() };
    let mut out = Vec::new();
    let mut args = [class, desc.into_bytes(), suffix].into_iter();
    let f = zstr::bytes(fmt);
    let mut i = 0;
    while i < f.len() {
        if f[i] == b'%' && i + 1 < f.len() && f[i + 1] == b's' {
            out.extend(args.next().unwrap_or_default());
            i += 2;
        } else {
            out.push(f[i]);
            i += 1;
        }
    }
    let o = call::this(ex);
    let mut a = [zval::string(&out), zval::long(0), zval::obj_ref(p)];
    let ctor = (*sys::zend_ce_error).constructor;
    let _ = call::call(&call::Callee { func: ctor, obj: o, scope: sys::zend_ce_error }, &mut a, null_mut());
    for z in a.iter_mut() {
        zval::dtor(z);
    }
}

// ---------------------------------------------------------------- ClosureHelper

/// ClosureHelper::getDescription(): "Scope::name defined in file:line" (as Reflection gives).
pub unsafe fn description(closure: *mut sys::zend_object) -> String {
    if closure.is_null() {
        return "???".into();
    }
    let f = sys::zend_get_closure_method_def(closure);
    if f.is_null() {
        return "???".into();
    }
    let mut d = zstr::str((*f).common.function_name).into_owned();
    let scope = (*f).common.scope;
    if !scope.is_null() {
        d = format!("{}::{d}", zstr::str((*scope).name));
    }
    if (*f).type_ as u32 == sys::ZEND_USER_FUNCTION {
        let op = &(*f).op_array;
        if !op.filename.is_null() && op.line_start != 0 {
            d = format!("{d} defined in {}:{}", zstr::str(op.filename), op.line_start);
        }
    }
    d
}

pub unsafe extern "C" fn closure_helper_get_description(ex: Ex, rv: *mut sys::zval) {
    let mut cl: *mut sys::zval = null_mut();
    if !zpp!(ex, "O", &mut cl, sys::zend_ce_closure) {
        return;
    }
    let d = description((*cl).value.obj);
    zval::write(rv, zval::string(d.as_bytes()));
}
