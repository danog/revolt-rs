//! Raw Zend Engine FFI: ext-php-rs's bindgen bindings (`ext_php_rs::ffi`, extended through
//! EXT_PHP_RS_ALLOWED_BINDINGS in .cargo/config.toml) plus declarations its wrapper header does
//! not include (zend_weakrefs.h, zend_closures.h).
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::all)]

pub use ext_php_rs::ffi::*;

// Windows: data exported by php8.dll is only reachable through its import library's `__imp_`
// symbols, which rustc uses for statics of an extern block linked to that library (as
// ext-php-rs's bindings are).
#[cfg_attr(all(windows, not(php_zts)), link(name = "php8"))]
#[cfg_attr(all(windows, php_zts), link(name = "php8ts"))]
extern "C" {
    pub static mut zend_ce_closure: *mut zend_class_entry;
    pub static mut zend_empty_string: *mut zend_string;
    pub static mut module_registry: HashTable;
    pub fn zend_fetch_debug_backtrace(return_value: *mut zval, skip_last: ::std::os::raw::c_int, options: ::std::os::raw::c_int, limit: ::std::os::raw::c_int);
    pub fn zend_weakrefs_hash_add(ht: *mut HashTable, key: *mut zend_object, data: *mut zval) -> *mut zval;
    pub fn zend_weakrefs_hash_del(ht: *mut HashTable, key: *mut zend_object) -> zend_result;
    pub fn zend_get_closure_method_def(obj: *mut zend_object) -> *const zend_function;
}

#[cfg(not(php86))]
extern "C" {
    pub fn zend_create_fake_closure(res: *mut zval, func: *mut zend_function, scope: *mut zend_class_entry, called_scope: *mut zend_class_entry, this_ptr: *mut zval);
}

#[cfg(php86)]
extern "C" {
    #[link_name = "zend_create_fake_closure"]
    fn zend_create_fake_closure_86(res: *mut zval, func: *mut zend_function, scope: *mut zend_class_entry, called_scope: *mut zend_class_entry, this_ptr: *mut zend_object);
}

/// PHP 8.6 passes the closure's `$this` as the object itself (before: a zval).
#[cfg(php86)]
pub unsafe fn zend_create_fake_closure(res: *mut zval, func: *mut zend_function, scope: *mut zend_class_entry, called_scope: *mut zend_class_entry, this_ptr: *mut zval) {
    let obj = if this_ptr.is_null() || (*this_ptr).u1.v.type_ != IS_OBJECT as u8 { std::ptr::null_mut() } else { (*this_ptr).value.obj };
    zend_create_fake_closure_86(res, func, scope, called_scope, obj)
}
