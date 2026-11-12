//! PHP streams watched by the loop: the OS handle behind a stream resource, and whether PHP's
//! own read buffer holds data (stream_select reports such streams readable at once).

use super::{sys, zval};

/// The OS handle of a stream (an fd on Unix; a SOCKET, or a CRT fd for other streams, on Windows).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Handle {
    pub fd: i64,
    /// Windows: the handle is a SOCKET (else a CRT file descriptor)
    pub socket: bool,
}

const AS_FD: i32 = 1;
#[cfg_attr(not(windows), allow(dead_code))]
const AS_SOCKETD: i32 = 2;
const AS_FD_FOR_SELECT: i32 = 3;
const CAST_INTERNAL: i32 = 0x20000000;

/// The php_stream of a stream resource zval, or null (not a stream, or closed).
pub unsafe fn of(z: *mut sys::zval) -> *mut sys::php_stream {
    let z = zval::deref(z);
    if zval::ty(z) != zval::RESOURCE {
        return std::ptr::null_mut();
    }
    let res = (*z).value.res;
    if (*res).type_ == sys::php_file_le_stream() || (*res).type_ == sys::php_file_le_pstream() {
        (*res).ptr as *mut sys::php_stream
    } else {
        std::ptr::null_mut()
    }
}

/// The handle to poll for a stream resource (stream_select's cast), or None.
pub unsafe fn handle(z: *mut sys::zval) -> Option<Handle> {
    let s = of(z);
    if s.is_null() {
        return None;
    }
    #[cfg(windows)]
    {
        let mut sock: usize = 0;
        if sys::_php_stream_cast(s, AS_SOCKETD | CAST_INTERNAL, &mut sock as *mut usize as *mut *mut _, 0) == sys::ZEND_RESULT_CODE_SUCCESS {
            return Some(Handle { fd: sock as i64, socket: true });
        }
    }
    let mut fd: i64 = -1;
    // php_stream_cast writes a php_socket_t (int on Unix) or an int fd
    let mut raw: [u8; 8] = [0xff; 8];
    for how in [AS_FD_FOR_SELECT, AS_FD] {
        if sys::_php_stream_cast(s, how | CAST_INTERNAL, raw.as_mut_ptr() as *mut *mut _, 0) == sys::ZEND_RESULT_CODE_SUCCESS {
            fd = i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64;
            break;
        }
    }
    if fd < 0 {
        return None;
    }
    Some(Handle { fd, socket: false })
}

/// Whether PHP's read buffer of the stream holds unread data.
#[inline]
pub unsafe fn has_buffered(z: *mut sys::zval) -> bool {
    let s = of(z);
    !s.is_null() && (*s).writepos > (*s).readpos
}

/// The resource of a stream zval (borrowed), or null.
pub unsafe fn resource(z: *mut sys::zval) -> *mut sys::zend_resource {
    let z = zval::deref(z);
    if zval::ty(z) == zval::RESOURCE {
        (*z).value.res
    } else {
        std::ptr::null_mut()
    }
}

/// The resource types of streams (le_stream, le_pstream), looked up once.
static mut LE: (i32, i32) = (0, 0);

/// Whether a stream resource (alive: someone holds it) has data in PHP's read buffer; false once
/// it was closed.
#[inline]
pub unsafe fn res_buffered(res: *mut sys::zend_resource) -> bool {
    let le = std::ptr::addr_of_mut!(LE);
    if (*le).0 == 0 {
        *le = (sys::php_file_le_stream(), sys::php_file_le_pstream());
    }
    let t = (*res).type_;
    if t != (*le).0 && t != (*le).1 {
        return false;
    }
    let s = (*res).ptr as *mut sys::php_stream;
    (*s).writepos > (*s).readpos
}
