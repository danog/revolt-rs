//! Weak object keys (zend_weakrefs_hash_add/del): a table whose entries the engine removes when
//! their key object is freed. Values are raw pointers (IS_PTR).

use super::{hash, sys, zval};
use std::ffi::c_void;
use std::ptr::null_mut;

pub struct WeakTable {
    ht: *mut sys::HashTable,
}

/// zend_object_to_weakref_key
#[inline(always)]
fn key(o: *mut sys::zend_object) -> u64 {
    (o as usize as u64) >> 3
}

impl WeakTable {
    pub fn new() -> WeakTable {
        WeakTable { ht: null_mut() }
    }

    pub fn get(&self, o: *mut sys::zend_object) -> *mut c_void {
        if self.ht.is_null() {
            return null_mut();
        }
        let z = unsafe { hash::find_index(self.ht, key(o)) };
        if z.is_null() {
            null_mut()
        } else {
            unsafe { (*z).value.ptr }
        }
    }

    pub fn add(&mut self, o: *mut sys::zend_object, p: *mut c_void) {
        unsafe {
            if self.ht.is_null() {
                self.ht = hash::new(8);
            }
            let mut z = zval::ptr(p);
            sys::zend_weakrefs_hash_add(self.ht, o, &mut *z);
        }
    }

    /// Removes `o`'s entry if it is there with value `p` (an entry of a freed object is gone
    /// already; another object at the same address has another value).
    pub fn remove(&mut self, o: *mut sys::zend_object, p: *mut c_void) {
        if !self.ht.is_null() && self.get(o) == p {
            unsafe { sys::zend_weakrefs_hash_del(self.ht, o) };
        }
    }

    /// Removes every entry (zend_weakrefs_hash_clean, which PHP 8.2/8.3 lack).
    pub fn clear(&mut self) {
        if self.ht.is_null() {
            return;
        }
        unsafe {
            let mut keys = Vec::new();
            hash::each(self.ht, |k, _| {
                if let hash::Key::Index(i) = k {
                    keys.push(i);
                }
                true
            });
            for k in keys {
                sys::zend_weakrefs_hash_del(self.ht, (k << 3) as usize as *mut sys::zend_object);
            }
            hash::release(self.ht);
        }
        self.ht = null_mut();
    }

    pub fn len(&self) -> u32 {
        if self.ht.is_null() {
            0
        } else {
            unsafe { hash::count(self.ht) }
        }
    }

    /// The values, in insertion order.
    pub fn values(&self) -> Vec<*mut c_void> {
        let mut v = Vec::new();
        if !self.ht.is_null() {
            unsafe {
                hash::each(self.ht, |_, z| {
                    v.push((*z).value.ptr);
                    true
                })
            };
        }
        v
    }
}

impl Drop for WeakTable {
    fn drop(&mut self) {
        self.clear();
    }
}
