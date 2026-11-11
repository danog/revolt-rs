//! zend_string helpers.

use super::sys;

/// A new (request-allocated) zend_string with a copy of `b`; refcount 1.
#[inline]
pub unsafe fn new(b: &[u8]) -> *mut sys::zend_string {
    sys::ext_php_rs_zend_string_init(b.as_ptr() as *const _, b.len(), false)
}

/// A persistent interned string (lives as long as the process; for class/method names).
pub unsafe fn interned(b: &[u8]) -> *mut sys::zend_string {
    let f = sys::zend_string_init_interned.expect("zend_string_init_interned");
    f(b.as_ptr() as *const _, b.len(), true)
}

#[inline(always)]
pub unsafe fn bytes<'a>(s: *const sys::zend_string) -> &'a [u8] {
    std::slice::from_raw_parts(std::ptr::addr_of!((*s).val) as *const u8, (*s).len)
}

#[inline(always)]
pub unsafe fn str<'a>(s: *const sys::zend_string) -> std::borrow::Cow<'a, str> {
    String::from_utf8_lossy(bytes(s))
}

#[inline(always)]
pub unsafe fn addref(s: *mut sys::zend_string) {
    if (*s).gc.u.type_info & sys::IS_STR_INTERNED == 0 {
        (*s).gc.refcount += 1;
    }
}

#[inline(always)]
pub unsafe fn release(s: *mut sys::zend_string) {
    sys::ext_php_rs_zend_string_release(s)
}
