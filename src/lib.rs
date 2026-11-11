//! revolt.so: the standalone revolt extension (revolt/event-loop in Rust). Everything is in the
//! revolt-rs crate; this is the module entry only.
#![cfg_attr(windows, feature(abi_vectorcall))]

use revolt_rs::module;
use revolt_rs::zend::sys;
use std::sync::OnceLock;

unsafe extern "C" fn minit(_type: i32, module_number: i32) -> sys::zend_result {
    module::minit(module_number);
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn mshutdown(_type: i32, _module_number: i32) -> sys::zend_result {
    module::mshutdown();
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn rinit(_type: i32, _module_number: i32) -> sys::zend_result {
    module::rinit();
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn rshutdown(_type: i32, _module_number: i32) -> sys::zend_result {
    module::rshutdown();
    sys::ZEND_RESULT_CODE_SUCCESS
}

unsafe extern "C" fn minfo(_module: *mut sys::zend_module_entry) {
    module::minfo();
}

struct Entry(sys::zend_module_entry);
unsafe impl Send for Entry {}
unsafe impl Sync for Entry {}

static ENTRY: OnceLock<Entry> = OnceLock::new();

/// The module entry PHP looks up when it loads the extension.
#[no_mangle]
pub extern "C" fn get_module() -> *mut sys::zend_module_entry {
    let e = ENTRY.get_or_init(|| Entry(module::module_entry(c"revolt", c"0.1.0", minit, mshutdown, rinit, rshutdown, minfo)));
    &e.0 as *const sys::zend_module_entry as *mut sys::zend_module_entry
}
