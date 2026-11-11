//! Preloaded ahead of the ASan runtime by `tools/asan.sh` on Linux. PHP before 8.5 loads its
//! extensions with `dlopen(RTLD_DEEPBIND)` (unless PHP itself is built with ASan), which ASan's
//! interceptor refuses (google/sanitizers#611); PHP 8.5 dropped the flag where `dlmopen` exists.
//! This `dlopen` comes first in the lookup order, drops the flag and calls the next one (ASan's).
#![cfg(target_os = "linux")]

use std::ffi::{c_char, c_int, c_void};

const RTLD_DEEPBIND: c_int = 0x8;
const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

type Dlopen = unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void;

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// # Safety
/// As `dlopen(3)`.
#[no_mangle]
pub unsafe extern "C" fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void {
    let next = dlsym(RTLD_NEXT, c"dlopen".as_ptr());
    if next.is_null() {
        return std::ptr::null_mut();
    }
    let next: Dlopen = std::mem::transmute(next);
    next(file, flags & !RTLD_DEEPBIND)
}
