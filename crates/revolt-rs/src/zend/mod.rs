//! Zend Engine layer: ext-php-rs's raw bindings (`sys`) plus Rust ports of the inline helpers
//! and macros of the PHP headers this extension needs (zend_types.h, zend_string.h,
//! zend_hash.h, zend_API.h, zend_fibers.h, zend_closures.h, zend_weakrefs.h, ...).
//!
//! Ownership: a `zval::Z` (never a bare `sys::zval`, which ext-php-rs gives a Drop) held by Rust
//! code owns one reference of its value; it is released
//! with [`zval::dtor`] (zval_ptr_dtor). Helpers taking `*const zval` borrow.
//!
//! Re-entrancy: any call into PHP (a closure, a method, a destructor through a refcount drop)
//! can re-enter the extension or bail out (longjmp) past Rust frames: no Rust borrow of shared
//! state is ever held across such a call, and nothing on such a frame needs dropping.

pub mod call;
pub mod class;
pub mod fiber;
pub mod hash;
pub mod obj;
pub mod stream;
pub mod sys;
pub mod weak;
pub mod zstr;
pub mod zval;

/// The executor globals (EG) of the calling thread.
#[inline(always)]
pub fn eg() -> *mut sys::zend_executor_globals {
    #[cfg(not(php_zts))]
    return std::ptr::addr_of_mut!(sys::executor_globals);
    // the thread's TSRM block plus the globals' offset (TSRMG_FAST_BULK, in ext-php-rs's wrapper.c)
    #[cfg(php_zts)]
    return unsafe { sys::ext_php_rs_executor_globals() };
}

/// The compiler globals (CG) of the calling thread.
#[inline(always)]
pub fn cg() -> *mut sys::zend_compiler_globals {
    #[cfg(not(php_zts))]
    return std::ptr::addr_of_mut!(sys::compiler_globals);
    #[cfg(php_zts)]
    return unsafe { sys::ext_php_rs_compiler_globals() };
}

/// The `zif_handler` of an `extern "C"` handler `$f`. PHP calls internal function handlers with
/// ZEND_FASTCALL, i.e. __vectorcall on Windows: there a vectorcall shim calls `$f`; elsewhere the
/// handler is `$f` itself.
#[cfg(windows)]
#[macro_export]
macro_rules! zh {
    ($f:path) => {{
        unsafe extern "vectorcall" fn shim(ex: *mut $crate::zend::sys::zend_execute_data, rv: *mut $crate::zend::sys::zval) {
            unsafe { $f(ex, rv) }
        }
        Some(shim as unsafe extern "vectorcall" fn(*mut $crate::zend::sys::zend_execute_data, *mut $crate::zend::sys::zval))
    }};
}

/// See the Windows definition.
#[cfg(not(windows))]
#[macro_export]
macro_rules! zh {
    ($f:path) => {
        Some($f as unsafe extern "C" fn(*mut $crate::zend::sys::zend_execute_data, *mut $crate::zend::sys::zval))
    };
}

/// Static engine tables (arrays of structs holding raw pointers): only read by PHP.
#[repr(transparent)]
pub struct Static<T>(pub T);
unsafe impl<T> Sync for Static<T> {}

/// EG(flags) & EG_FLAGS_IN_SHUTDOWN
pub fn in_shutdown() -> bool {
    unsafe { (*eg()).flags & 1 != 0 }
}

/// The running fiber (EG(active_fiber)) as its object, or null in {main}.
#[inline(always)]
pub fn active_fiber() -> *mut sys::zend_object {
    let f = unsafe { (*eg()).active_fiber };
    if f.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { std::ptr::addr_of_mut!((*f).std) }
    }
}

/// gc_collect_cycles(): the number of collected values.
pub fn gc_collect_cycles() -> i32 {
    unsafe { sys::zend_gc_collect_cycles() }
}

/// getenv() as PHP's: the SAPI's environment first (FastCGI params), then the process's.
pub fn getenv(name: &str) -> Option<String> {
    unsafe {
        let mut n = name.as_bytes().to_vec();
        n.push(0);
        let v = sys::sapi_getenv(n.as_ptr() as *mut _, name.len());
        if !v.is_null() {
            let s = std::ffi::CStr::from_ptr(v).to_string_lossy().into_owned();
            sys::_efree(v as *mut _);
            return Some(s);
        }
    }
    std::env::var(name).ok()
}

extern "C" {
    // variadic: not emitted usefully by bindgen
    pub fn php_info_print_table_header(num_cols: i32, ...);
    pub fn php_info_print_table_row(num_cols: i32, ...);
}
