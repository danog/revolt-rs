//! zval construction, inspection and release (zend_types.h macros).

use super::sys;
use std::ptr::null_mut;

/// A zval held by value. ext-php-rs gives `sys::zval` a Drop (it releases the value): ours are
/// plain, released explicitly ([`dtor`]) or by a [`Zv`].
pub type Z = std::mem::ManuallyDrop<sys::zval>;

/// Anything that designates a zval: raw pointers, references, our `Z`.
pub trait AsZ {
    fn zp(self) -> *mut sys::zval;
}
impl AsZ for *mut sys::zval {
    #[inline(always)]
    fn zp(self) -> *mut sys::zval {
        self
    }
}
impl AsZ for *const sys::zval {
    #[inline(always)]
    fn zp(self) -> *mut sys::zval {
        self as *mut _
    }
}
impl AsZ for &mut sys::zval {
    #[inline(always)]
    fn zp(self) -> *mut sys::zval {
        self
    }
}
impl AsZ for &sys::zval {
    #[inline(always)]
    fn zp(self) -> *mut sys::zval {
        self as *const _ as *mut _
    }
}
impl AsZ for &mut Z {
    #[inline(always)]
    fn zp(self) -> *mut sys::zval {
        &mut **self
    }
}
impl AsZ for &Z {
    #[inline(always)]
    fn zp(self) -> *mut sys::zval {
        &**self as *const _ as *mut _
    }
}

pub const UNDEF: u8 = sys::IS_UNDEF as u8;
pub const NULL: u8 = sys::IS_NULL as u8;
pub const FALSE: u8 = sys::IS_FALSE as u8;
pub const TRUE: u8 = sys::IS_TRUE as u8;
pub const LONG: u8 = sys::IS_LONG as u8;
pub const DOUBLE: u8 = sys::IS_DOUBLE as u8;
pub const STRING: u8 = sys::IS_STRING as u8;
pub const ARRAY: u8 = sys::IS_ARRAY as u8;
pub const OBJECT: u8 = sys::IS_OBJECT as u8;
pub const RESOURCE: u8 = sys::IS_RESOURCE as u8;
pub const REFERENCE: u8 = sys::IS_REFERENCE as u8;
pub const PTR: u8 = sys::IS_PTR as u8;

/// Z_TYPE_FLAGS: refcounted | collectable, as type_info bits
const REFCOUNTED: u32 = 1 << 8;
const COLLECTABLE: u32 = 1 << 9;

#[inline(always)]
fn with(type_info: u32) -> Z {
    let mut z: Z = unsafe { std::mem::zeroed() };
    z.u1.type_info = type_info;
    z
}

#[inline(always)]
pub fn undef() -> Z {
    with(UNDEF as u32)
}

#[inline(always)]
pub fn null() -> Z {
    with(NULL as u32)
}

#[inline(always)]
pub fn bool(b: bool) -> Z {
    with(if b { TRUE } else { FALSE } as u32)
}

#[inline(always)]
pub fn long(v: i64) -> Z {
    let mut z = with(LONG as u32);
    z.value.lval = v;
    z
}

#[inline(always)]
pub fn double(v: f64) -> Z {
    let mut z = with(DOUBLE as u32);
    z.value.dval = v;
    z
}

#[inline(always)]
pub fn ptr(p: *mut std::ffi::c_void) -> Z {
    let mut z = with(PTR as u32);
    z.value.ptr = p;
    z
}

/// A zval owning a reference to `s` (taken over, not added).
#[inline(always)]
pub unsafe fn str_owned(s: *mut sys::zend_string) -> Z {
    let interned = (*s).gc.u.type_info & sys::IS_STR_INTERNED != 0;
    let mut z = with(if interned { STRING as u32 } else { STRING as u32 | REFCOUNTED });
    z.value.str_ = s;
    z
}

/// A new zval string with a copy of `b`.
pub unsafe fn string(b: &[u8]) -> Z {
    str_owned(super::zstr::new(b))
}

/// A zval holding a new reference to `s`.
#[inline(always)]
pub unsafe fn str_ref(s: *mut sys::zend_string) -> Z {
    super::zstr::addref(s);
    str_owned(s)
}

/// A zval owning a reference to `o` (taken over, not added).
#[inline(always)]
pub unsafe fn obj_owned(o: *mut sys::zend_object) -> Z {
    let mut z = with(OBJECT as u32 | REFCOUNTED | COLLECTABLE);
    z.value.obj = o;
    z
}

/// A zval holding a new reference to `o`.
#[inline(always)]
pub unsafe fn obj_ref(o: *mut sys::zend_object) -> Z {
    (*o).gc.refcount += 1;
    obj_owned(o)
}

/// A zval owning a reference to `a` (taken over).
#[inline(always)]
pub unsafe fn arr_owned(a: *mut sys::zend_array) -> Z {
    let immutable = (*a).gc.u.type_info & sys::GC_IMMUTABLE != 0;
    let mut z = with(if immutable { ARRAY as u32 } else { ARRAY as u32 | REFCOUNTED | COLLECTABLE });
    z.value.arr = a;
    z
}

/// A new empty array.
pub unsafe fn empty_array() -> Z {
    arr_owned(super::hash::new(0))
}

#[inline(always)]
pub fn ty(z: impl AsZ) -> u8 {
    unsafe { (*z.zp()).u1.v.type_ }
}

#[inline(always)]
pub fn is_undef(z: impl AsZ) -> bool {
    ty(z) == UNDEF
}

#[inline(always)]
pub fn is_null(z: impl AsZ) -> bool {
    ty(z) == NULL
}

#[inline(always)]
pub fn is_refcounted(z: impl AsZ) -> bool {
    unsafe { (*z.zp()).u1.type_info & REFCOUNTED != 0 }
}

/// The value behind a reference (ZVAL_DEREF).
#[inline(always)]
pub unsafe fn deref(z: impl AsZ) -> *mut sys::zval {
    let z = z.zp();
    if ty(z) == REFERENCE {
        std::ptr::addr_of_mut!((*(*z).value.ref_).val)
    } else {
        z
    }
}

#[inline(always)]
pub unsafe fn obj(z: impl AsZ) -> *mut sys::zend_object {
    let z = z.zp();
    if ty(z) == OBJECT {
        (*z).value.obj
    } else {
        null_mut()
    }
}

/// A new owned copy of `*z` (ZVAL_COPY: adds a reference).
#[inline(always)]
pub unsafe fn copy(z: impl AsZ) -> Z {
    let z = z.zp();
    let mut c: Z = std::mem::ManuallyDrop::new(std::ptr::read(z));
    if is_refcounted(z) {
        (*c.value.counted).gc.refcount += 1;
    }
    c.u2.extra = 0;
    c
}

/// ZVAL_COPY_DEREF.
#[inline(always)]
pub unsafe fn copy_deref(z: impl AsZ) -> Z {
    copy(deref(z))
}

/// Releases one reference (zval_ptr_dtor) and leaves UNDEF.
#[inline(always)]
pub unsafe fn dtor(z: impl AsZ) {
    let z = z.zp();
    if is_refcounted(z) {
        // decrements, destroys at 0, else buffers a possible cycle root
        sys::zval_ptr_dtor(z);
    }
    (*z).u1.type_info = UNDEF as u32;
}

/// Releases a reference to `o` taken for the duration of a call (a pair whose net effect is
/// none): just a decrement while others remain, no possible-root buffering (whoever released the
/// other references buffered it already if it may be garbage); the full release otherwise.
#[inline(always)]
pub unsafe fn obj_unref_pair(o: *mut sys::zend_object) {
    if (*o).gc.refcount > 1 {
        (*o).gc.refcount -= 1;
    } else {
        let mut z = obj_owned(o);
        dtor(&mut z);
    }
}

/// A zval borrowing `o` (no reference taken: never released, only passed to engine functions
/// that read it).
#[inline(always)]
pub unsafe fn obj_borrowed(o: *mut sys::zend_object) -> Z {
    obj_owned(o)
}

/// Releases and moves out: `*z` becomes UNDEF, the value is returned owned.
#[inline(always)]
pub unsafe fn take(z: impl AsZ) -> Z {
    let z = z.zp();
    let v = std::mem::ManuallyDrop::new(std::ptr::read(z));
    (*z).u1.type_info = UNDEF as u32;
    v
}

/// Writes an owned value into `*dst` (no release of the old value).
#[inline(always)]
pub unsafe fn write(dst: impl AsZ, v: Z) {
    let dst = dst.zp();
    (*dst).value = v.value;
    (*dst).u1.type_info = v.u1.type_info;
}

/// An owned zval holder that releases its value when dropped.
#[repr(transparent)]
pub struct Zv(pub Z);

impl Zv {
    #[inline(always)]
    pub fn undef() -> Zv {
        Zv(undef())
    }
    #[inline(always)]
    pub fn null() -> Zv {
        Zv(null())
    }
    #[inline(always)]
    pub fn as_ptr(&mut self) -> *mut sys::zval {
        &mut *self.0
    }
    #[inline(always)]
    pub fn ty(&self) -> u8 {
        ty(&*self.0)
    }
    #[inline(always)]
    pub fn obj(&self) -> *mut sys::zend_object {
        unsafe { obj(&*self.0) }
    }
    /// Takes the value out (the holder becomes UNDEF).
    #[inline(always)]
    pub fn take(&mut self) -> Z {
        unsafe { take(&mut *self.0) }
    }
    /// Replaces the value, releasing the old one.
    #[inline(always)]
    pub fn set(&mut self, v: Z) {
        unsafe {
            let mut old = take(&mut self.0);
            write(&mut self.0, v);
            dtor(&mut old);
        }
    }
    #[inline(always)]
    pub fn clone_ref(&self) -> Zv {
        Zv(unsafe { copy(&*self.0) })
    }
}

impl Drop for Zv {
    #[inline(always)]
    fn drop(&mut self) {
        unsafe { dtor(&mut *self.0) }
    }
}
