//! The hooks a host extension calls from its module entry: the standalone `revolt` extension
//! (src/lib.rs of the workspace root) or any extension embedding revolt-rs.
//!
//! The first copy of revolt-rs whose MINIT finds no `Revolt\EventLoop` class registers all the
//! classes and owns the loop; later copies (another embedder, the standalone extension, possibly
//! another revolt-rs version) register nothing and reach the owner through `crate::embed`.

use crate::classes;
use crate::zend::{self, sys};
use std::sync::atomic::{AtomicBool, Ordering};

/// The Zend module API number and build id of the PHP this was built for.
pub const ZEND_MODULE_API_NO: i32 = parse(env!("REVOLT_RS_PHP_API"));
pub const BUILD_ID: &str = concat!(env!("REVOLT_RS_PHP_BUILD_ID"), "\0");
/// The version of revolt-rs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The revolt/event-loop version whose API and behaviour this implements (composer `provide`).
pub const REVOLT_VERSION: &str = "1.0.9";

const fn parse(s: &str) -> i32 {
    let b = s.as_bytes();
    let mut i = 0;
    let mut n = 0i32;
    while i < b.len() {
        n = n * 10 + (b[i] - b'0') as i32;
        i += 1;
    }
    n
}

static OWNER: AtomicBool = AtomicBool::new(false);

/// Whether this copy registered the classes (and runs the loop).
pub fn is_owner() -> bool {
    OWNER.load(Ordering::Relaxed)
}

/// MINIT: registers the Revolt classes unless another copy did; returns whether this copy owns
/// them.
pub unsafe fn minit(_module_number: i32) -> bool {
    crate::core::backend::minit();
    zend::fiber::minit();
    let ct = (*zend::cg()).class_table;
    if !zend::hash::find_str(ct, b"revolt\\eventloop").is_null() {
        return false;
    }
    classes::register_all();
    classes::errors::minit();
    OWNER.store(true, Ordering::Relaxed);
    crate::embed::publish();
    true
}

pub unsafe fn rinit() {}

/// RSHUTDOWN: the thread's driver and fiber-local storage go (destructors already ran).
pub unsafe fn rshutdown() {
    if is_owner() {
        classes::eventloop::rshutdown();
        classes::fiber_local::rshutdown();
    }
}

pub unsafe fn mshutdown() {}

/// phpinfo() rows.
pub unsafe fn minfo() {
    sys::php_info_print_table_start();
    zend::php_info_print_table_header(2, c"revolt-rs (native revolt/event-loop)".as_ptr(), c"enabled".as_ptr());
    let row = |k: &std::ffi::CStr, v: &str| {
        let v = std::ffi::CString::new(v).unwrap();
        zend::php_info_print_table_row(2, k.as_ptr(), v.as_ptr());
    };
    row(c"revolt-rs version", VERSION);
    row(c"revolt/event-loop API", REVOLT_VERSION);
    row(c"Classes registered by", if is_owner() { "this module" } else { "another module (revolt-rs copy)" });
    #[cfg(target_os = "linux")]
    row(c"Reactor", "epoll");
    #[cfg(all(unix, not(target_os = "linux")))]
    row(c"Reactor", "kqueue");
    #[cfg(windows)]
    row(c"Reactor", "WSAPoll");
    sys::php_info_print_table_end();
}

/// A module entry for a host extension named `name` (NUL-terminated) with these hooks wired:
/// the standalone `revolt` extension uses it as is; an embedder calls the hooks from its own.
pub fn module_entry(
    name: &'static std::ffi::CStr,
    version: &'static std::ffi::CStr,
    minit: unsafe extern "C" fn(i32, i32) -> sys::zend_result,
    mshutdown: unsafe extern "C" fn(i32, i32) -> sys::zend_result,
    rinit: unsafe extern "C" fn(i32, i32) -> sys::zend_result,
    rshutdown: unsafe extern "C" fn(i32, i32) -> sys::zend_result,
    minfo: unsafe extern "C" fn(*mut sys::zend_module_entry),
) -> sys::zend_module_entry {
    sys::zend_module_entry {
        size: std::mem::size_of::<sys::zend_module_entry>() as u16,
        zend_api: ZEND_MODULE_API_NO as u32,
        zend_debug: 0,
        zts: cfg!(php_zts) as u8,
        ini_entry: std::ptr::null(),
        deps: std::ptr::null(),
        name: name.as_ptr(),
        functions: std::ptr::null(),
        module_startup_func: Some(minit),
        module_shutdown_func: Some(mshutdown),
        request_startup_func: Some(rinit),
        request_shutdown_func: Some(rshutdown),
        info_func: Some(minfo),
        version: version.as_ptr(),
        globals_size: 0,
        #[cfg(not(php_zts))]
        globals_ptr: std::ptr::null_mut(),
        #[cfg(php_zts)]
        globals_id_ptr: std::ptr::null_mut(),
        globals_ctor: None,
        globals_dtor: None,
        post_deactivate_func: None,
        module_started: 0,
        type_: 0,
        handle: std::ptr::null_mut(),
        module_number: 0,
        build_id: BUILD_ID.as_ptr() as *const std::ffi::c_char,
    }
}
