//! PHP fibers driven from Rust: Fiber objects whose function is a Closure over a static internal
//! function, started/resumed/suspended through zend_fiber_start/resume/suspend (PHP 8.4+, with
//! the checks the Fiber methods make first) or through the Fiber methods themselves (8.2/8.3).

use super::{call, sys, zstr, zval, zval::Zv};
use std::ptr::null_mut;

pub const STATUS_INIT: u32 = 0;
pub const STATUS_RUNNING: u32 = 1;
pub const STATUS_SUSPENDED: u32 = 2;
pub const STATUS_DEAD: u32 = 3;
/// ZEND_FIBER_FLAG_DESTROYED
const FLAG_DESTROYED: u32 = 1 << 2;

static mut FIBER_ERROR: *mut sys::zend_class_entry = null_mut();

/// MINIT: FiberError (its class entry is not exported).
pub unsafe fn minit() {
    FIBER_ERROR = call::lookup_class(b"fibererror");
}

pub fn fiber_error_ce() -> *mut sys::zend_class_entry {
    unsafe { FIBER_ERROR }
}

#[inline(always)]
unsafe fn zf(o: *mut sys::zend_object) -> *mut sys::zend_fiber {
    // zend_fiber starts with its zend_object
    o as *mut sys::zend_fiber
}

#[inline(always)]
pub unsafe fn status(o: *mut sys::zend_object) -> u32 {
    (*zf(o)).context.status as u32
}

/// Fiber::isTerminated()
#[inline(always)]
pub unsafe fn is_terminated(o: *mut sys::zend_object) -> bool {
    status(o) == STATUS_DEAD
}

/// Whether the fiber was destroyed while suspended (its stack unwound by the engine).
#[inline(always)]
pub unsafe fn is_destroyed(o: *mut sys::zend_object) -> bool {
    (*zf(o)).flags as u32 & FLAG_DESTROYED != 0
}

/// A static internal function usable as a Closure (for fiber bodies and callbacks): leaked.
pub unsafe fn internal_function(name: &str, handler: sys::zif_handler, scope: *mut sys::zend_class_entry) -> *mut sys::zend_function {
    let f: *mut sys::zend_function = Box::leak(Box::new(std::mem::zeroed::<sys::zend_function>()));
    let i = std::ptr::addr_of_mut!((*f).internal_function);
    (*i).type_ = sys::ZEND_INTERNAL_FUNCTION as u8;
    (*i).fn_flags = sys::ZEND_ACC_PUBLIC | super::class::STRICT;
    (*i).function_name = zstr::interned(name.as_bytes());
    (*i).scope = scope;
    (*i).handler = handler;
    f
}

/// A Closure over `func` with `$this` = `this` (a new reference) or none.
pub unsafe fn closure(func: *mut sys::zend_function, this: *mut sys::zend_object) -> Zv {
    let mut rv = Zv::undef();
    let mut t = if this.is_null() { zval::undef() } else { zval::obj_ref(this) };
    let scope = (*func).common.scope;
    sys::zend_create_fake_closure(rv.as_ptr(), func, scope, if this.is_null() { scope } else { (*this).ce }, if this.is_null() { null_mut() } else { &mut *t });
    zval::dtor(&mut t);
    rv
}

/// new Fiber($callable): the object (owned), or null with an exception.
pub unsafe fn new(callable: &mut Zv) -> *mut sys::zend_object {
    call::new_object(sys::zend_ce_fiber, std::slice::from_mut(&mut callable.0))
}

#[cfg(php84)]
unsafe fn blocked() -> bool {
    if sys::zend_fiber_switch_blocked() {
        sys::zend_throw_error(FIBER_ERROR, c"Cannot switch fibers in current execution context".as_ptr());
        return true;
    }
    false
}

/// $fiber->start(): the value of its first suspension (or its return), UNDEF on exception.
pub unsafe fn start(o: *mut sys::zend_object) -> Zv {
    #[cfg(php84)]
    {
        let mut rv = Zv::undef();
        if blocked() {
            return rv;
        }
        if status(o) != STATUS_INIT {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot start a fiber that has already been started".as_ptr());
            return rv;
        }
        let f = zf(o);
        (*f).fci.params = null_mut();
        (*f).fci.param_count = 0;
        (*f).fci.named_params = null_mut();
        if sys::zend_fiber_start(f, rv.as_ptr()) != sys::ZEND_RESULT_CODE_SUCCESS {
            return Zv::undef();
        }
        rv
    }
    #[cfg(not(php84))]
    call::method(o, b"start", &mut [])
}

/// $fiber->resume($value)
pub unsafe fn resume(o: *mut sys::zend_object, value: &mut zval::Z) -> Zv {
    #[cfg(php84)]
    {
        let mut rv = Zv::undef();
        if blocked() {
            return rv;
        }
        let f = zf(o);
        if status(o) != STATUS_SUSPENDED || !(*f).caller.is_null() {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot resume a fiber that is not suspended".as_ptr());
            return rv;
        }
        sys::zend_fiber_resume(f, &mut **value, rv.as_ptr());
        rv
    }
    #[cfg(not(php84))]
    call::method(o, b"resume", std::slice::from_mut(value))
}

/// $fiber->throw($exception)
pub unsafe fn throw_into(o: *mut sys::zend_object, ex: &mut zval::Z) -> Zv {
    call::method(o, b"throw", std::slice::from_mut(ex))
}

/// Fiber::suspend($value) from the running fiber: the value it is resumed with (UNDEF when an
/// exception was thrown into it, or when it could not suspend).
pub unsafe fn suspend(value: &mut zval::Z) -> Zv {
    #[cfg(php84)]
    {
        let mut rv = Zv::undef();
        let f = (*super::eg()).active_fiber;
        if f.is_null() {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot suspend outside of a fiber".as_ptr());
            return rv;
        }
        if (*f).flags as u32 & FLAG_DESTROYED != 0 {
            sys::zend_throw_error(FIBER_ERROR, c"Cannot suspend in a force-closed fiber".as_ptr());
            return rv;
        }
        if blocked() {
            return rv;
        }
        sys::zend_fiber_suspend(f, &mut **value, rv.as_ptr());
        rv
    }
    #[cfg(not(php84))]
    {
        let suspend = call::find_method(sys::zend_ce_fiber, b"suspend");
        call::call(&call::Callee { func: suspend, obj: null_mut(), scope: sys::zend_ce_fiber }, std::slice::from_mut(value), null_mut())
    }
}

// ---------------------------------------------------------------- values held across a call

/// Values the extension holds while a call made from a fiber runs (the callback, its arguments):
/// PHP keeps such values in the frames of the fiber, where the cycle collector sees them; ours
/// are reported by the fiber's get_gc, so a fiber suspended in a callback and only referenced
/// from a cycle is collected as Revolt's would be. Per fiber, a stack of frames (nested calls
/// hold and take in LIFO order); fibers made by [`new_tracked`], or any fiber after [`track`]
/// (the embedding API holds values for user fibers too). Key 0 is {main} (never collected).
pub mod inflight {
    use super::super::{sys, zval};
    use std::cell::UnsafeCell;
    use std::collections::HashMap;
    use std::ffi::c_int;
    use std::ptr::null_mut;

    /// The held values of one fiber: its frames back to back, `marks` the start of each frame.
    /// Kept (with their capacity) as long as the fiber lives: holds allocate nothing once warm.
    #[derive(Default)]
    struct Frames {
        vals: Vec<zval::Z>,
        marks: Vec<usize>,
    }

    /// Fiber object => its frames (boxed: stable while the map grows), and the last one used
    /// (a fiber holds and takes many times in a row: no lookup then).
    struct Held {
        map: HashMap<usize, Box<Frames>, std::hash::BuildHasherDefault<crate::core::IdHasher>>,
        last_key: usize,
        last: *mut Frames,
    }

    thread_local! {
        static HELD: UnsafeCell<Held> = UnsafeCell::new(Held { map: HashMap::default(), last_key: usize::MAX, last: null_mut() });
    }

    /// The thread's table. Never kept across a call into PHP (which may hold, take, or free a
    /// fiber): every function here takes it anew.
    #[inline(always)]
    fn held() -> *mut Held {
        HELD.with(|h| h.get())
    }

    /// The frames of `key`, made if needed.
    #[inline(always)]
    unsafe fn frames(key: usize) -> *mut Frames {
        let h = &mut *held();
        if h.last_key == key {
            return h.last;
        }
        let f: *mut Frames = &mut **h.map.entry(key).or_default();
        h.last_key = key;
        h.last = f;
        f
    }

    /// The frames of `key` if there are any.
    #[inline(always)]
    unsafe fn find(key: usize) -> *mut Frames {
        let h = &mut *held();
        if h.last_key == key {
            return h.last;
        }
        match h.map.get_mut(&key) {
            Some(f) => &mut **f,
            None => null_mut(),
        }
    }

    static mut HANDLERS: *mut sys::zend_object_handlers = null_mut();
    static mut ORIG_GC: sys::zend_object_get_gc_t = None;
    static mut ORIG_FREE: sys::zend_object_free_obj_t = None;

    /// Our handlers for fiber objects (the engine's, with get_gc and free_obj wrapped).
    pub unsafe fn handlers(of: *mut sys::zend_object) -> *const sys::zend_object_handlers {
        if HANDLERS.is_null() {
            let mut h: sys::zend_object_handlers = std::ptr::read((*of).handlers);
            ORIG_GC = h.get_gc;
            ORIG_FREE = h.free_obj;
            h.get_gc = Some(get_gc);
            h.free_obj = Some(free_obj);
            HANDLERS = Box::leak(Box::new(h));
        }
        HANDLERS
    }

    /// Makes the values held for fiber `f` (a \Fiber, not null) visible to the cycle collector.
    pub unsafe fn track(f: *mut sys::zend_object) {
        if (*f).handlers != HANDLERS as *const _ {
            (*f).handlers = handlers(f);
        }
    }

    /// Holds `values` (owned) for the running fiber `f` (null: {main}) until the matching
    /// [`take`]; returns a pointer to them (the parameters of a call: valid until the next hold
    /// or take on this thread, so a call must be made right away).
    #[inline]
    pub fn hold<I: IntoIterator<Item = zval::Z>>(f: *mut sys::zend_object, values: I) -> (*mut zval::Z, usize) {
        unsafe {
            let fr = &mut *frames(f as usize);
            let start = fr.vals.len();
            fr.marks.push(start);
            fr.vals.extend(values);
            (fr.vals.as_mut_ptr().add(start), fr.vals.len() - start)
        }
    }

    /// The values of the last [`hold`] for `f` (null: {main}), as [`hold`] returned them (they
    /// may have moved since: holds of nested calls).
    #[inline]
    pub fn params(f: *mut sys::zend_object) -> (*mut zval::Z, usize) {
        unsafe {
            let fr = find(f as usize);
            if fr.is_null() {
                return (null_mut(), 0);
            }
            let fr = &mut *fr;
            match fr.marks.last() {
                Some(&start) => (fr.vals.as_mut_ptr().add(start), fr.vals.len() - start),
                None => (null_mut(), 0),
            }
        }
    }

    /// The values of the last [`hold`] for `f` (null: {main}), owned by the caller again.
    #[inline]
    pub fn take(f: *mut sys::zend_object) -> Taken {
        let mut out = Taken { len: 0, inline: unsafe { std::mem::zeroed() }, heap: Vec::new() };
        unsafe {
            let fr = find(f as usize);
            if fr.is_null() {
                return out;
            }
            let fr = &mut *fr;
            let Some(start) = fr.marks.pop() else { return out };
            let n = fr.vals.len() - start;
            out.len = n;
            if n <= INLINE {
                std::ptr::copy_nonoverlapping(fr.vals.as_ptr().add(start), out.inline.as_mut_ptr(), n);
                fr.vals.set_len(start);
            } else {
                out.heap = fr.vals.split_off(start);
            }
            if fr.marks.is_empty() && !f.is_null() && (*f).handlers != HANDLERS as *const _ {
                // a fiber whose free_obj is not ours: its entry must not outlive it
                drop(remove(f as usize));
            }
        }
        out
    }

    const INLINE: usize = 6;

    /// The values of a frame (owned: each is released explicitly, or left UNDEF).
    pub struct Taken {
        len: usize,
        inline: [zval::Z; INLINE],
        heap: Vec<zval::Z>,
    }

    impl std::ops::Deref for Taken {
        type Target = [zval::Z];
        fn deref(&self) -> &[zval::Z] {
            if self.len <= INLINE { &self.inline[..self.len] } else { &self.heap }
        }
    }

    impl std::ops::DerefMut for Taken {
        fn deref_mut(&mut self) -> &mut [zval::Z] {
            if self.len <= INLINE { &mut self.inline[..self.len] } else { &mut self.heap }
        }
    }

    unsafe extern "C" fn get_gc(o: *mut sys::zend_object, table: *mut *mut sys::zval, n: *mut c_int) -> *mut sys::HashTable {
        let ht = ORIG_GC.unwrap()(o, table, n);
        let fr = find(o as usize);
        if fr.is_null() || (*fr).vals.is_empty() {
            return ht;
        }
        let buf = std::ptr::addr_of_mut!((*super::super::eg()).get_gc_buffer);
        if *table != (*buf).start {
            // not the engine's buffer: leave the report as it is
            return ht;
        }
        (*buf).cur = (*buf).start.add(*n as usize);
        // (no PHP code runs here: the frames stay put)
        for z in (*fr).vals.iter_mut() {
            let z: *mut sys::zval = &mut **z;
            if zval::is_refcounted(z) {
                if (*buf).cur == (*buf).end {
                    sys::zend_get_gc_buffer_grow(buf);
                }
                std::ptr::copy_nonoverlapping(z, (*buf).cur, 1);
                (*buf).cur = (*buf).cur.add(1);
            }
        }
        *table = (*buf).start;
        *n = (*buf).cur.offset_from((*buf).start) as c_int;
        ht
    }

    /// The frames of `key` leave the table (their values are the caller's).
    unsafe fn remove(key: usize) -> Option<Box<Frames>> {
        let h = &mut *held();
        if h.last_key == key {
            h.last_key = usize::MAX;
            h.last = null_mut();
        }
        h.map.remove(&key)
    }

    /// RSHUTDOWN: whatever is still held (a frame of a fiber that never resumed) goes.
    pub unsafe fn rshutdown() {
        let keys: Vec<usize> = (*held()).map.keys().copied().collect();
        for k in keys {
            if let Some(fr) = remove(k) {
                for mut z in fr.vals {
                    zval::dtor(&mut z);
                }
            }
        }
    }

    unsafe extern "C" fn free_obj(o: *mut sys::zend_object) {
        ORIG_FREE.unwrap()(o);
        if let Some(fr) = remove(o as usize) {
            for mut z in fr.vals {
                zval::dtor(&mut z);
            }
        }
    }
}

/// new Fiber($callable) whose in-flight values are reported to the cycle collector.
pub unsafe fn new_tracked(callable: &mut Zv) -> *mut sys::zend_object {
    let f = new(callable);
    if !f.is_null() {
        (*f).handlers = inflight::handlers(f);
    }
    f
}
