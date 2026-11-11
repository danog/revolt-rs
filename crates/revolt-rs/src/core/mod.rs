//! The native event loop behind every `AbstractDriver` object: callback records, the enable
//! queues, the callback queue, microtasks, the interrupt, the timer heap and the reactor.
//!
//! The loop logic (ticks, the callback fiber, errors, suspensions) is in `run`; the OS reactor
//! in `backend`. Everything here mirrors revolt/event-loop's AbstractDriver: the comments name
//! the PHP code each piece stands for.

pub mod backend;
pub mod clock;
pub mod run;
pub mod signal;
pub mod timer;

use crate::zend::{call::Callee, stream::Handle, sys, weak::WeakTable, zstr, zval, zval::Zv};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::hash::{BuildHasherDefault, Hasher};
use std::ptr::null_mut;

/// Hashes callback numbers (sequential integers) for the record map.
#[derive(Default)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    #[inline(always)]
    fn write_u64(&mut self, n: u64) {
        self.0 = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type IdMap<V> = HashMap<u64, V, BuildHasherDefault<IdHasher>>;

/// Revolt's callback ids are "a", "b", ..., "z", "aa", ... (str_increment from "a"): bijective
/// base 26, so a number stands for each id. 1 is "a".
pub fn id_to_string(mut n: u64) -> Vec<u8> {
    let mut s = Vec::with_capacity(4);
    while n > 0 {
        n -= 1;
        s.push(b'a' + (n % 26) as u8);
        n /= 26;
    }
    s.reverse();
    s
}

/// The number of an id string; None for strings that are no id (never valid identifiers).
#[inline]
pub fn string_to_id(s: &[u8]) -> Option<u64> {
    if s.is_empty() || s.len() > 13 {
        return None;
    }
    let mut n: u64 = 0;
    for &c in s {
        if !c.is_ascii_lowercase() {
            return None;
        }
        n = n.checked_mul(26)?.checked_add((c - b'a' + 1) as u64)?;
    }
    Some(n)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Defer,
    Delay,
    Repeat,
    Readable,
    Writable,
    Signal,
}

/// A callback function: a PHP Closure, or (embedding API) a Rust function.
pub type NativeFn = unsafe extern "C" fn(data: *mut c_void, id: u64);
pub type ReleaseFn = unsafe extern "C" fn(data: *mut c_void);

pub enum Cb {
    Php { closure: Zv, callee: Callee },
    Native { f: NativeFn, data: *mut c_void, release: Option<ReleaseFn> },
    /// moved out for an invocation (a fired defer/delay)
    Taken,
}

impl Drop for Cb {
    fn drop(&mut self) {
        if let Cb::Native { data, release: Some(r), .. } = *self {
            unsafe { r(data) };
        }
    }
}

/// DriverCallback (and its subclasses) as a Rust record.
pub struct Record {
    pub id: u64,
    pub id_str: *mut sys::zend_string,
    pub kind: Kind,
    pub cb: Cb,
    /// StreamCallback::$stream
    pub stream: Zv,
    /// the OS handle behind `stream`, resolved at activation
    pub handle: Option<Handle>,
    pub signal: i32,
    pub interval: f64,
    pub expiration: f64,
    pub enabled: bool,
    pub invokable: bool,
    pub referenced: bool,
    /// enable queue membership: (queued, generation of the queue entry)
    pub queued: bool,
    pub qgen: u32,
    /// activated in the reactor (fd registered / timer in the heap / signal watched)
    pub active: bool,
    /// position in the timer heap (usize::MAX: not in it)
    pub heap_pos: usize,
    /// the DriverCallback object handed to PHP code (userland drivers), or null
    pub view: *mut sys::zend_object,
}

impl Record {
    pub fn closure_obj(&self) -> *mut sys::zend_object {
        match &self.cb {
            Cb::Php { closure, .. } => closure.obj(),
            _ => null_mut(),
        }
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        unsafe {
            zstr::release(self.id_str);
            if !self.view.is_null() {
                let mut z = zval::obj_owned(self.view);
                zval::dtor(&mut z);
            }
        }
    }
}

/// Where an error came from (UncaughtThrowable's "callback %s"): a Closure, or a description.
pub enum Desc {
    Closure(Zv),
    Static(&'static str),
}

/// What `{main}` gets from the loop (Revolt's interrupt closures).
pub enum Interrupt {
    /// a `{main}` suspension resumed with a value
    Value(Zv),
    /// a `{main}` suspension resumed with an exception
    Throw(Zv),
    /// an exception no error handler took: UncaughtThrowable::throwingCallback (or
    /// throwingErrorHandler when `handler`), unless it is an UncaughtThrowable already
    Uncaught { desc: Desc, ex: Zv, handler: bool },
}

pub enum Microtask {
    Php { closure: Zv, args: Vec<Zv>, named: Zv },
    /// DriverSuspension::resume()/throw() of a fiber suspension
    Resume { fiber: Zv, value: Zv, throw: bool },
    Native { f: NativeFn, data: *mut c_void },
}

/// The loop's place, so that it can return to `{main}` and continue later (Revolt suspends its
/// loop fiber instead).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// not running (Revolt: no loop fiber, or a terminated one)
    Idle,
    /// invokeCallbacks() before the first tick
    First,
    /// the top of the `while (!$this->stopped)` loop
    Top,
    /// invokeCallbacks() inside tick(), before dispatch(true)
    TickInvoke,
    /// invokeCallbacks() after tick()
    AfterTick,
}

/// The class a driver object is of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DriverKind {
    StreamSelect,
    Ev,
    Event,
    Uv,
    /// EventLoop::setDriver()'s temporary driver during gc_collect_cycles()
    Gc,
    /// a PHP class extending AbstractDriver
    User,
}

/// The PHP methods of a userland driver class the core must call (null: the native one).
#[derive(Default)]
pub struct Hooks {
    pub activate: Option<*mut sys::zend_function>,
    pub deactivate: Option<*mut sys::zend_function>,
    pub dispatch: Option<*mut sys::zend_function>,
    pub now: Option<*mut sys::zend_function>,
    pub cancel: Option<*mut sys::zend_function>,
    pub enable: Option<*mut sys::zend_function>,
    pub disable: Option<*mut sys::zend_function>,
    /// Driver methods overridden in PHP (bits: `crate::classes::driver::OVR_*`)
    pub overrides: u64,
}

pub struct Core {
    /// the driver object (not a reference: the Core lives inside it)
    pub this: *mut sys::zend_object,
    pub kind: DriverKind,
    pub hooks: Hooks,
    pub next_id: u64,
    pub callbacks: IdMap<Box<Record>>,
    /// $enableQueue / $enableDeferQueue: (id, generation) in order; stale entries skipped
    pub enable_queue: Vec<(u64, u32)>,
    pub enable_queue_len: usize,
    pub defer_queue: Vec<(u64, u32)>,
    pub defer_queue_len: usize,
    pub callback_queue: VecDeque<u64>,
    pub microtasks: VecDeque<Microtask>,
    pub error_handler: Zv,
    pub interrupt: Option<Interrupt>,
    pub idle: bool,
    pub stopped: bool,
    pub phase: Phase,
    /// the frame of the call running the loop (Revolt: the loop fiber is running), or null
    pub in_loop: *mut sys::zend_execute_data,
    /// the native callback being invoked (its release waits until it returns)
    pub in_call: u64,
    pub ready_buf: Vec<backend::Ready>,
    pub signals: signal::Watchers,
    /// tick()'s $blocking while its invokeCallbacks() runs
    pub tick_blocking: bool,
    /// enabled and referenced callbacks (isEmpty() is `== 0`)
    pub live: usize,
    pub callback_fiber: Zv,
    /// the fiber that last suspended with the internal marker
    pub yielded: *mut sys::zend_object,
    /// the suspensions of fibers (weak keys), and `{main}`'s (cleared when it is freed)
    pub suspensions: WeakTable,
    pub main_suspension: *mut sys::zend_object,
    /// the handler and the exception of an error-handler fiber about to start
    pub error_stash: Option<(Zv, Zv, Desc)>,
    /// native callbacks released after their invocation (cancelled while running)
    pub deferred_release: Vec<Cb>,
    /// getHandle()'s value (EvLoop, EventBase, uv loop), created on first use
    pub handle: Zv,
    pub reactor: backend::Reactor,
    pub timers: timer::Heap,
}

impl Core {
    pub fn new(kind: DriverKind) -> Core {
        Core {
            this: null_mut(),
            kind,
            hooks: Hooks::default(),
            next_id: 1,
            callbacks: IdMap::default(),
            enable_queue: Vec::new(),
            enable_queue_len: 0,
            defer_queue: Vec::new(),
            defer_queue_len: 0,
            callback_queue: VecDeque::new(),
            microtasks: VecDeque::new(),
            error_handler: Zv::null(),
            interrupt: None,
            idle: false,
            stopped: false,
            phase: Phase::Idle,
            in_loop: null_mut(),
            in_call: 0,
            ready_buf: Vec::new(),
            signals: signal::Watchers::default(),
            tick_blocking: false,
            live: 0,
            callback_fiber: Zv::undef(),
            yielded: null_mut(),
            suspensions: WeakTable::new(),
            main_suspension: null_mut(),
            error_stash: None,
            deferred_release: Vec::new(),
            handle: Zv::undef(),
            reactor: backend::Reactor::new(),
            timers: timer::Heap::default(),
        }
    }

    /// Reports every zval held to the cycle collector.
    pub fn gc(&mut self, add: &mut dyn FnMut(*mut sys::zval)) {
        for r in self.callbacks.values_mut() {
            if let Cb::Php { closure, .. } = &mut r.cb {
                add(&mut *closure.0);
            }
            add(&mut *r.stream.0);
            if !r.view.is_null() {
                let mut z = unsafe { zval::obj_owned(r.view) };
                add(&mut *z);
                // the buffer copies the zval: nothing to release here
                std::mem::forget(z);
            }
        }
        for t in self.microtasks.iter_mut() {
            match t {
                Microtask::Php { closure, args, named } => {
                    add(&mut *closure.0);
                    for a in args.iter_mut() {
                        add(&mut *a.0);
                    }
                    add(&mut *named.0);
                }
                Microtask::Resume { fiber, value, .. } => {
                    add(&mut *fiber.0);
                    add(&mut *value.0);
                }
                Microtask::Native { .. } => {}
            }
        }
        add(&mut *self.error_handler.0);
        match &mut self.interrupt {
            Some(Interrupt::Value(v)) | Some(Interrupt::Throw(v)) => add(&mut *v.0),
            Some(Interrupt::Uncaught { desc, ex, .. }) => {
                add(&mut *ex.0);
                if let Desc::Closure(c) = desc {
                    add(&mut *c.0);
                }
            }
            None => {}
        }
        if let Some((h, e, _)) = &mut self.error_stash {
            add(&mut *h.0);
            add(&mut *e.0);
        }
        add(&mut *self.callback_fiber.0);
        add(&mut *self.handle.0);
    }

    /// A new record with the next id.
    pub fn new_record(&mut self, kind: Kind, cb: Cb) -> &mut Record {
        let id = self.next_id;
        self.next_id += 1;
        let s = id_to_string(id);
        let rec = Box::new(Record {
            id,
            id_str: unsafe { zstr::new(&s) },
            kind,
            cb,
            stream: Zv::undef(),
            handle: None,
            signal: 0,
            interval: 0.0,
            expiration: 0.0,
            enabled: true,
            invokable: false,
            referenced: true,
            queued: false,
            qgen: 0,
            active: false,
            heap_pos: usize::MAX,
            view: null_mut(),
        });
        self.live += 1;
        self.callbacks.entry(id).or_insert(rec)
    }

    #[inline]
    pub fn record(&mut self, id: u64) -> Option<&mut Record> {
        self.callbacks.get_mut(&id).map(|b| &mut **b)
    }

    /// Adds to $enableQueue (or $enableDeferQueue for defers).
    pub fn queue_enable(&mut self, id: u64) {
        let Some(r) = self.callbacks.get_mut(&id) else { return };
        r.queued = true;
        r.qgen = r.qgen.wrapping_add(1);
        let entry = (id, r.qgen);
        if r.kind == Kind::Defer {
            self.defer_queue.push(entry);
            self.defer_queue_len += 1;
        } else {
            self.enable_queue.push(entry);
            self.enable_queue_len += 1;
        }
    }

    /// unset($this->enableQueue[$id]) / unset($this->enableDeferQueue[$id])
    pub fn unqueue_enable(&mut self, id: u64) -> bool {
        let Some(r) = self.callbacks.get_mut(&id) else { return false };
        if !r.queued {
            return false;
        }
        r.queued = false;
        if r.kind == Kind::Defer {
            self.defer_queue_len -= 1;
        } else {
            self.enable_queue_len -= 1;
        }
        true
    }

    /// Drains an enable queue: the ids still queued, in order (marked not queued).
    pub fn take_queue(&mut self, defer: bool) -> Vec<u64> {
        let q = std::mem::take(if defer { &mut self.defer_queue } else { &mut self.enable_queue });
        if defer {
            self.defer_queue_len = 0;
        } else {
            self.enable_queue_len = 0;
        }
        let mut out = Vec::with_capacity(q.len());
        for (id, g) in q {
            if let Some(r) = self.callbacks.get_mut(&id) {
                if r.queued && r.qgen == g {
                    r.queued = false;
                    out.push(id);
                }
            }
        }
        out
    }

    pub fn set_enabled(&mut self, id: u64, on: bool) {
        if let Some(r) = self.callbacks.get_mut(&id) {
            if r.enabled != on {
                r.enabled = on;
                if r.referenced {
                    if on {
                        self.live += 1;
                    } else {
                        self.live -= 1;
                    }
                }
                view_set_bool(r.view, ViewProp::Enabled, on);
            }
        }
    }

    pub fn set_referenced(&mut self, id: u64, on: bool) {
        if let Some(r) = self.callbacks.get_mut(&id) {
            if r.referenced != on {
                r.referenced = on;
                if r.enabled {
                    if on {
                        self.live += 1;
                    } else {
                        self.live -= 1;
                    }
                }
                view_set_bool(r.view, ViewProp::Referenced, on);
            }
        }
    }

    pub fn set_invokable(&mut self, id: u64, on: bool) {
        if let Some(r) = self.callbacks.get_mut(&id) {
            r.invokable = on;
            view_set_bool(r.view, ViewProp::Invokable, on);
        }
    }

    /// unset($this->callbacks[$id]) (after disable()).
    pub fn remove(&mut self, id: u64) -> Option<Box<Record>> {
        let mut r = self.callbacks.remove(&id)?;
        if r.enabled && r.referenced {
            self.live -= 1;
        }
        if r.heap_pos != usize::MAX {
            let pos = r.heap_pos;
            r.heap_pos = usize::MAX;
            self.timers.remove(&mut self.callbacks, pos);
        }
        Some(r)
    }

    /// enqueueCallback()
    #[inline]
    pub fn enqueue(&mut self, id: u64) {
        self.callback_queue.push_back(id);
        self.idle = false;
    }

    /// The ids of all callbacks, in creation order (getIdentifiers()).
    pub fn ids(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.callbacks.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // release the records (and their native data) before the reactor goes
        self.callbacks.clear();
        self.microtasks.clear();
        self.suspensions.clear();
    }
}

/// The DriverCallback properties written through from the records.
#[derive(Clone, Copy)]
pub enum ViewProp {
    Invokable,
    Enabled,
    Referenced,
    Expiration,
}

pub fn view_set_bool(view: *mut sys::zend_object, p: ViewProp, v: bool) {
    if !view.is_null() {
        unsafe { crate::classes::callbacks::view_write(view, p, zval::bool(v)) };
    }
}
