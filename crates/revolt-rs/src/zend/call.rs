//! Function-call plumbing: the arguments of an internal call (ZEND_CALL_ARG, ZEND_NUM_ARGS,
//! getThis), parameter parsing (fast paths for the exact types, zend_parse_parameters for
//! everything else: coercion, strict_types, PHP's own error messages), exceptions, and calls of
//! closures, functions and methods.

use super::{eg, sys, zstr, zval, zval::Zv};
use std::ffi::{c_char, CStr, CString};
use std::ptr::null_mut;

pub type Ex = *mut sys::zend_execute_data;

const FRAME_SLOT: usize = std::mem::size_of::<sys::zend_execute_data>().div_ceil(std::mem::size_of::<sys::zval>());
const ZEND_CALL_HAS_EXTRA_NAMED_PARAMS: u32 = 1 << 27;

extern "C" {
    pub fn zend_parse_parameters(num_args: u32, type_spec: *const c_char, ...) -> sys::zend_result;
    fn zend_is_unwind_exit(ex: *const sys::zend_object) -> bool;
    fn zend_is_graceful_exit(ex: *const sys::zend_object) -> bool;
}

#[inline(always)]
pub unsafe fn num_args(ex: Ex) -> u32 {
    (*ex).This.u2.num_args
}

/// ZEND_CALL_ARG(ex, n + 1)
#[inline(always)]
pub unsafe fn arg(ex: Ex, n: usize) -> *mut sys::zval {
    (ex as *mut sys::zval).add(FRAME_SLOT + n)
}

#[inline(always)]
pub unsafe fn this(ex: Ex) -> *mut sys::zend_object {
    (*ex).This.value.obj
}

/// The named arguments a variadic internal function collected (EX(extra_named_params)).
#[inline(always)]
pub unsafe fn extra_named(ex: Ex) -> *mut sys::HashTable {
    if (*ex).This.u1.type_info & ZEND_CALL_HAS_EXTRA_NAMED_PARAMS != 0 {
        (*ex).extra_named_params
    } else {
        null_mut()
    }
}

/// zend_parse_parameters_none(): false (an ArgumentCountError is thrown) when args were given.
#[inline(always)]
pub unsafe fn none(ex: Ex) -> bool {
    if num_args(ex) != 0 {
        sys::zend_wrong_parameters_none_error();
        return false;
    }
    true
}

#[inline(always)]
pub fn has_exception() -> bool {
    unsafe { !(*eg()).exception.is_null() }
}

/// Runs zend_parse_parameters with `spec`: true when the arguments are fine (else an exception
/// was thrown). The callers pass out-pointers matching the spec.
#[macro_export]
macro_rules! zpp {
    ($ex:expr, $spec:literal $(, $out:expr)* $(,)?) => {
        $crate::zend::call::parse($crate::zend::call::num_args($ex), concat!($spec, "\0").as_ptr() as *const _, |n, s| unsafe {
            $crate::zend::call::zpp_raw(n, s $(, $out)*)
        })
    };
}

#[doc(hidden)]
pub unsafe fn parse(n: u32, spec: *const c_char, f: impl FnOnce(u32, *const c_char) -> sys::zend_result) -> bool {
    f(n, spec) == sys::ZEND_RESULT_CODE_SUCCESS
}

#[doc(hidden)]
pub use zend_parse_parameters as zpp_raw;

/// The Closure object of argument `n` when it is exactly a Closure (fast path).
#[inline(always)]
pub unsafe fn closure_arg(ex: Ex, n: usize) -> *mut sys::zend_object {
    let z = arg(ex, n);
    if zval::ty(z) == zval::OBJECT && (*(*z).value.obj).ce == sys::zend_ce_closure {
        (*z).value.obj
    } else {
        null_mut()
    }
}

/// The string of argument `n` when it is exactly a string (fast path).
#[inline(always)]
pub unsafe fn str_arg(ex: Ex, n: usize) -> *mut sys::zend_string {
    let z = arg(ex, n);
    if zval::ty(z) == zval::STRING {
        (*z).value.str_
    } else {
        null_mut()
    }
}

/// zend_throw_exception(ce, msg, code); the message stops at a NUL as with C strings.
pub unsafe fn throw(ce: *mut sys::zend_class_entry, msg: &str) {
    let c = cs(msg);
    sys::zend_throw_exception(ce, c.as_ptr(), 0);
}

/// throw new \Error($msg)
pub unsafe fn throw_error(msg: &str) {
    throw(sys::zend_ce_error, msg)
}

/// A C string of a Rust string (cut at the first NUL).
pub fn cs(s: &str) -> CString {
    let b = s.as_bytes();
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    CString::new(&b[..end]).unwrap()
}

/// Takes the pending exception (EG(exception)) out of the engine: the caller owns it.
pub unsafe fn take_exception() -> *mut sys::zend_object {
    let e = (*eg()).exception;
    (*eg()).exception = null_mut();
    e
}

/// Re-throws an exception object (owned reference taken over).
pub unsafe fn rethrow(e: *mut sys::zend_object) {
    sys::zend_throw_exception_object(&mut *zval::obj_owned(e));
}

/// Whether the pending exception is an exit() / a fiber's destruction unwinding the stack:
/// never caught by catch blocks, so never by the event loop either.
pub unsafe fn exception_is_unwind() -> bool {
    let e = (*eg()).exception;
    !e.is_null() && (zend_is_unwind_exit(e) || zend_is_graceful_exit(e))
}

/// The function, `$this` and called scope a callable object (a Closure) stands for.
#[derive(Clone, Copy)]
pub struct Callee {
    pub func: *mut sys::zend_function,
    pub obj: *mut sys::zend_object,
    pub scope: *mut sys::zend_class_entry,
}

/// The callee of a Closure object (its get_closure handler); None for an object that is not
/// callable.
#[inline]
pub unsafe fn callee_of(o: *mut sys::zend_object) -> Option<Callee> {
    let h = (*(*o).handlers).get_closure?;
    let mut c = Callee { func: null_mut(), obj: null_mut(), scope: null_mut() };
    if h(o, &mut c.scope, &mut c.func, &mut c.obj, false) != sys::ZEND_RESULT_CODE_SUCCESS {
        return None;
    }
    Some(c)
}

/// Calls `c` with `params` (borrowed: the callee gets its own references) and the optional named
/// parameters; returns the result (UNDEF when the call failed or threw).
#[inline]
pub unsafe fn call(c: &Callee, params: &mut [zval::Z], named: *mut sys::HashTable) -> Zv {
    let mut rv = Zv::undef();
    let mut fci: sys::zend_fcall_info = std::mem::zeroed();
    fci.size = std::mem::size_of::<sys::zend_fcall_info>();
    fci.function_name = std::mem::ManuallyDrop::into_inner(zval::undef());
    fci.retval = rv.as_ptr();
    fci.params = if params.is_empty() { null_mut() } else { params.as_mut_ptr() as *mut sys::zval };
    fci.param_count = params.len() as u32;
    fci.object = c.obj;
    fci.named_params = named;
    let mut fcc: sys::zend_fcall_info_cache = std::mem::zeroed();
    fcc.function_handler = c.func;
    fcc.object = c.obj;
    fcc.called_scope = c.scope;
    fcc.calling_scope = c.scope;
    sys::zend_call_function(&mut fci, &mut fcc);
    rv
}

/// Calls a Closure object.
pub unsafe fn call_closure(o: *mut sys::zend_object, params: &mut [zval::Z]) -> Zv {
    match callee_of(o) {
        Some(c) => call(&c, params, null_mut()),
        None => Zv::undef(),
    }
}

/// The method `lcname` (lower case) of class `ce`, or null.
pub unsafe fn find_method(ce: *mut sys::zend_class_entry, lcname: &[u8]) -> *mut sys::zend_function {
    let z = super::hash::find_str(&(*ce).function_table, lcname);
    if z.is_null() {
        null_mut()
    } else {
        (*z).value.func
    }
}

/// $o->method(...$params) by lower-case name; UNDEF when the call failed.
pub unsafe fn method(o: *mut sys::zend_object, lcname: &[u8], params: &mut [zval::Z]) -> Zv {
    let f = find_method((*o).ce, lcname);
    if f.is_null() {
        // __call and friends: through the engine's own lookup
        let mut obj = zval::obj_ref(o);
        let mut name = zval::string(lcname);
        let mut rv = Zv::undef();
        sys::_call_user_function_impl(&mut *obj, &mut *name, rv.as_ptr(), params.len() as u32, params.as_mut_ptr() as *mut sys::zval, null_mut());
        zval::dtor(&mut name);
        zval::dtor(&mut obj);
        return rv;
    }
    call(&Callee { func: f, obj: o, scope: (*o).ce }, params, null_mut())
}

/// A static method of `ce` by lower-case name.
pub unsafe fn static_method(ce: *mut sys::zend_class_entry, lcname: &[u8], params: &mut [zval::Z]) -> Zv {
    let f = find_method(ce, lcname);
    if f.is_null() {
        return Zv::undef();
    }
    call(&Callee { func: f, obj: null_mut(), scope: ce }, params, null_mut())
}

/// A registered class by lower-case name, without autoloading; null when missing.
pub unsafe fn lookup_class(lcname: &[u8]) -> *mut sys::zend_class_entry {
    // CG(class_table): EG(class_table) is not set yet at MINIT
    let ct = (*super::cg()).class_table;
    let z = super::hash::find_str(ct, lcname);
    if z.is_null() {
        null_mut()
    } else {
        (*z).value.ce
    }
}

/// A class by name with autoloading (zend_lookup_class).
pub unsafe fn lookup_class_autoload(name: &[u8]) -> *mut sys::zend_class_entry {
    let s = zstr::new(name);
    let ce = sys::zend_lookup_class_ex(s, null_mut(), 0);
    zstr::release(s);
    ce
}

/// instanceof_function
#[inline(always)]
pub unsafe fn instance_of(ce: *const sys::zend_class_entry, of: *const sys::zend_class_entry) -> bool {
    ce == of || sys::instanceof_function_slow(ce, of)
}

/// `new $ce(...$params)`: the object, or null with an exception thrown.
pub unsafe fn new_object(ce: *mut sys::zend_class_entry, params: &mut [zval::Z]) -> *mut sys::zend_object {
    let mut z = zval::undef();
    if sys::object_init_ex(&mut *z, ce) != sys::ZEND_RESULT_CODE_SUCCESS {
        return null_mut();
    }
    let o = z.value.obj;
    let ctor = (*ce).constructor;
    if !ctor.is_null() {
        let _ = call(&Callee { func: ctor, obj: o, scope: ce }, params, null_mut());
        if has_exception() {
            zval::dtor(&mut z);
            return null_mut();
        }
    }
    o
}

/// The name of a class entry.
pub unsafe fn class_name<'a>(ce: *const sys::zend_class_entry) -> &'a [u8] {
    zstr::bytes((*ce).name)
}

/// The C string of a static name, for engine calls.
pub fn c(s: &'static CStr) -> *const c_char {
    s.as_ptr()
}
