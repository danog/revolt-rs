//! HashTable helpers (zend_hash.h): creation, insertion, lookup and iteration of both the
//! packed (zval list, PHP 8.2+) and the hash (Bucket) layouts.

use super::{sys, zstr, zval};
use std::ptr::null_mut;

/// HASH_FLAG_PACKED
const PACKED: u32 = 1 << 2;

/// A new empty array (refcount 1).
pub unsafe fn new(size: u32) -> *mut sys::zend_array {
    sys::_zend_new_array(size)
}

#[inline]
pub unsafe fn count(ht: *const sys::HashTable) -> u32 {
    (*ht).nNumOfElements
}

/// Appends `v` (taken over).
pub unsafe fn push(ht: *mut sys::HashTable, mut v: zval::Z) {
    sys::zend_hash_next_index_insert(ht, &mut *v);
}

/// $ht[$key] = v (taken over).
pub unsafe fn set_str(ht: *mut sys::HashTable, key: &[u8], mut v: zval::Z) {
    sys::zend_hash_str_update(ht, key.as_ptr() as *const _, key.len(), &mut *v);
}

/// $ht[$key] = v with a zend_string key (taken over: v; borrowed: key).
pub unsafe fn set_zstr(ht: *mut sys::HashTable, key: *mut sys::zend_string, mut v: zval::Z) {
    sys::zend_hash_update(ht, key, &mut *v);
}

pub unsafe fn set_index(ht: *mut sys::HashTable, key: u64, mut v: zval::Z) {
    sys::zend_hash_index_update(ht, key, &mut *v);
}

pub unsafe fn find_str(ht: *const sys::HashTable, key: &[u8]) -> *mut sys::zval {
    sys::zend_hash_str_find(ht, key.as_ptr() as *const _, key.len())
}

pub unsafe fn find_index(ht: *const sys::HashTable, key: u64) -> *mut sys::zval {
    sys::zend_hash_index_find(ht, key)
}

/// A key of an array element.
pub enum Key {
    Index(u64),
    Str(*mut sys::zend_string),
}

/// Calls `f` for each element in order (UNDEF slots skipped); stops when `f` returns false.
/// `f` must not modify the table.
pub unsafe fn each(ht: *const sys::HashTable, mut f: impl FnMut(Key, *mut sys::zval) -> bool) {
    let used = (*ht).nNumUsed;
    if (*ht).u.flags & PACKED != 0 {
        let base = (*ht).__bindgen_anon_1.arPacked;
        for i in 0..used {
            let z = base.add(i as usize);
            if !zval::is_undef(z) && !f(Key::Index(i as u64), z) {
                return;
            }
        }
    } else {
        let base = (*ht).__bindgen_anon_1.arData;
        for i in 0..used {
            let b = base.add(i as usize);
            let z = std::ptr::addr_of_mut!((*b).val);
            if zval::is_undef(z) {
                continue;
            }
            let k = if (*b).key.is_null() { Key::Index((*b).h) } else { Key::Str((*b).key) };
            if !f(k, z) {
                return;
            }
        }
    }
}

/// Releases a reference of an array (zend_array_release).
pub unsafe fn release(a: *mut sys::zend_array) {
    let mut z = zval::arr_owned(a);
    zval::dtor(&mut z);
}

/// The string key as bytes (empty for an integer key).
pub unsafe fn key_bytes<'a>(k: &Key) -> Option<&'a [u8]> {
    match k {
        Key::Str(s) => Some(zstr::bytes(*s)),
        Key::Index(_) => None,
    }
}

#[allow(dead_code)]
fn _unused() -> *mut sys::zval {
    null_mut()
}
