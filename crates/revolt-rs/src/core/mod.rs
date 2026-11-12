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

/// Hashes integer keys (callback numbers, fds, object addresses) with one multiplication; the
/// high half is folded into the low one (hashbrown picks buckets with the low bits, which an
/// aligned address alone would leave zero).
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
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
    #[inline(always)]
    fn write_i64(&mut self, n: i64) {
        self.write_u64(n as u64);
    }
    #[inline(always)]
    fn write_i32(&mut self, n: i32) {
        self.write_u64(n as u64);
    }
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0 ^ (self.0 >> 32)
    }
}

pub type IdMap<V> = HashMap<u64, V, BuildHasherDefault<IdHasher>>;

/// Revolt's callback ids are "a", "b", ..., "z", "aa", ... (str_increment from "a"): bijective
/// base 26, so a number stands for each id. 1 is "a".
pub fn id_to_string(n: u64) -> Vec<u8> {
    id_bytes(n).as_bytes().to_vec()
}

/// An id's string without allocating (at most 14 letters: u64::MAX in bijective base 26).
pub struct IdBuf {
    buf: [u8; 14],
    start: usize,
}

impl IdBuf {
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[self.start..]
    }
}

/// [`id_to_string`] on the stack.
#[inline]
pub fn id_bytes(mut n: u64) -> IdBuf {
    let mut b = IdBuf { buf: [0; 14], start: 14 };
    while n > 0 {
        n -= 1;
        b.start -= 1;
        b.buf[b.start] = b'a' + (n % 26) as u8;
        n /= 26;
    }
    b
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
    /// `held`: an object the callback keeps alive (one reference), reported to the cycle
    /// collector (e.g. the suspension the callback resumes); released after `release`
    Native { f: NativeFn, data: *mut c_void, release: Option<ReleaseFn>, held: Zv },
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

/// A callback's id and the slot of its record (valid while `records.at_id()` finds it: a slot is
/// reused by later records, ids never are).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cid {
    pub id: u64,
    pub slot: u32,
}

/// DriverCallback (and its subclasses) as a Rust record. Kept within 128 bytes (two cache lines,
/// asserted below): what only one kind of callback uses goes in `watch`.
pub struct Record {
    pub id: u64,
    /// its slot in the record table
    pub slot: u32,
    /// the id's string, made when first needed (PHP callbacks: at once, it is returned; native
    /// ones: only if PHP code asks); null until then
    id_str: *mut sys::zend_string,
    pub kind: Kind,
    pub cb: Cb,
    /// what the callback waits for (the data of its kind)
    pub watch: Watch,
    pub enabled: bool,
    pub invokable: bool,
    pub referenced: bool,
    /// enable queue membership: (queued, generation of the queue entry)
    pub queued: bool,
    pub qgen: u32,
    /// activated in the reactor (fd registered / timer in the heap / signal watched)
    pub active: bool,
    /// the DriverCallback object handed to PHP code (userland drivers), or null
    pub view: *mut sys::zend_object,
}

const _: () = assert!(size_of::<Record>() <= 128);
const _: () = assert!(size_of::<Option<Record>>() <= 128);

/// The data of a callback's kind.
pub enum Watch {
    Defer,
    /// Delay, Repeat
    Timer { interval: f64, expiration: f64 },
    /// Readable, Writable
    Stream {
        /// StreamCallback::$stream (null: a raw OS handle of the embedding API)
        stream: Zv,
        /// the OS handle behind `stream`, resolved at activation
        handle: Option<Handle>,
        /// its index in the reactor's `php_readers` (u32::MAX: not there)
        reader_idx: u32,
    },
    Signal(i32),
}

impl Record {
    #[inline(always)]
    pub fn cid(&self) -> Cid {
        Cid { id: self.id, slot: self.slot }
    }

    /// The id's string (borrowed: owned by the record).
    #[inline]
    pub fn id_zstr(&mut self) -> *mut sys::zend_string {
        if self.id_str.is_null() {
            self.id_str = unsafe { zstr::new(id_bytes(self.id).as_bytes()) };
        }
        self.id_str
    }

    /// The id's string, owned by the caller from now on (the record has none then).
    #[inline]
    pub fn take_id_zstr(&mut self) -> *mut sys::zend_string {
        let s = self.id_zstr();
        self.id_str = null_mut();
        s
    }

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
            if !self.id_str.is_null() {
                zstr::release(self.id_str);
            }
            if !self.view.is_null() {
                let mut z = zval::obj_owned(self.view);
                zval::dtor(&mut z);
            }
        }
    }
}

/// The records of a driver: a slab (slots reused, no allocation per callback once warm; grown
/// by chunks, so records never move) and the id => slot map for the operations by id (PHP
/// passes id strings). After a burst (many callbacks, then few), the empty chunks at the end and
/// the map's excess capacity are given back.
#[derive(Default)]
pub struct Records {
    chunks: Vec<Box<[Option<Record>; CHUNK]>>,
    /// the records in each chunk (empty chunks are skipped by iterations)
    live: Vec<u8>,
    /// slots never used yet start here
    fresh: u32,
    free: Vec<u32>,
    map: IdMap<u32>,
    /// the free list's length that makes `remove` try to give memory back
    compact_at: usize,
}

const CHUNK: usize = 64;
/// below this many free slots nothing is given back
const KEEP: usize = 4096;

impl Records {
    #[inline(always)]
    fn slot(&mut self, slot: u32) -> &mut Option<Record> {
        &mut self.chunks[slot as usize / CHUNK][slot as usize % CHUNK]
    }

    #[inline(always)]
    fn slot_ref(&self, slot: u32) -> &Option<Record> {
        &self.chunks[slot as usize / CHUNK][slot as usize % CHUNK]
    }

    /// Stores a record (its `slot` is set); returns it.
    #[inline]
    pub fn insert(&mut self, mut r: Record) -> &mut Record {
        let slot = match self.free.pop() {
            Some(s) => s,
            None => {
                if self.fresh as usize == self.chunks.len() * CHUNK {
                    self.chunks.push(Box::new(std::array::from_fn(|_| None)));
                    self.live.push(0);
                }
                self.fresh += 1;
                self.fresh - 1
            }
        };
        r.slot = slot;
        self.map.insert(r.id, slot);
        self.live[slot as usize / CHUNK] += 1;
        self.slot(slot).insert(r)
    }

    #[inline]
    pub fn get(&mut self, id: u64) -> Option<&mut Record> {
        let slot = *self.map.get(&id)?;
        self.slot(slot).as_mut()
    }

    #[inline]
    pub fn get_ref(&self, id: u64) -> Option<&Record> {
        let slot = *self.map.get(&id)?;
        self.slot_ref(slot).as_ref()
    }

    /// The record of `c` if it still exists (no lookup).
    #[inline(always)]
    pub fn at_id(&mut self, c: Cid) -> Option<&mut Record> {
        match self.chunks.get_mut(c.slot as usize / CHUNK).map(|ch| &mut ch[c.slot as usize % CHUNK]) {
            Some(Some(r)) if r.id == c.id => Some(r),
            _ => None,
        }
    }

    /// The record in `slot` (which must hold one).
    #[inline(always)]
    pub fn at(&mut self, slot: u32) -> &mut Record {
        // callers pass slots of live records (the timer heap's)
        self.slot(slot).as_mut().unwrap()
    }

    /// Takes the record `id` out (one move: its owner drops it outside of the table, since
    /// dropping it may run PHP code).
    #[inline]
    pub fn remove(&mut self, id: u64) -> Option<Record> {
        let slot = self.map.remove(&id)?;
        self.free.push(slot);
        self.live[slot as usize / CHUNK] -= 1;
        let r = self.slot(slot).take();
        let free = self.free.len();
        if free > KEEP && (self.map.is_empty() || (free > self.compact_at && free > 4 * self.map.len())) {
            self.compact();
        }
        r
    }

    /// Gives back the empty chunks at the end and the map's excess capacity (after a burst).
    #[cold]
    fn compact(&mut self) {
        while self.live.last() == Some(&0) {
            self.live.pop();
            self.chunks.pop();
        }
        let end = (self.chunks.len() * CHUNK) as u32;
        self.fresh = self.fresh.min(end);
        self.free.retain(|&s| s < end);
        self.free.shrink_to(self.free.len() * 2);
        self.map.shrink_to(self.map.len() * 2);
        // nothing more to give back until the free list doubles
        self.compact_at = self.free.len() * 2;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn values(&self) -> impl Iterator<Item = &Record> {
        self.chunks.iter().zip(&self.live).filter(|(_, &n)| n > 0).flat_map(|(c, _)| c.iter()).flatten()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Record> {
        self.chunks.iter_mut().zip(&self.live).filter(|(_, &n)| n > 0).flat_map(|(c, _)| c.iter_mut()).flatten()
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.free.clear();
        self.live.clear();
        self.fresh = 0;
        // dropped in slot order (records may run release hooks)
        drop(std::mem::take(&mut self.chunks));
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
    /// the closure, its named arguments (an array, or UNDEF) and the number of its positional
    /// ones, which wait in `Core::microtask_args` (in queue order: no allocation per microtask)
    Php { closure: Zv, nargs: u32, named: Zv },
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
    /// Driver\NativeDriver
    Native,
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
    pub callbacks: Records,
    /// $enableQueue / $enableDeferQueue: (callback, generation) in order; stale entries skipped
    pub enable_queue: Vec<(Cid, u32)>,
    pub enable_queue_len: usize,
    pub defer_queue: Vec<(Cid, u32)>,
    pub defer_queue_len: usize,
    /// a drained enable queue (its buffer kept: no allocation per tick)
    pub spare_queue: Vec<(Cid, u32)>,
    pub callback_queue: VecDeque<Cid>,
    /// tick()'s buffer of drained enable queues (kept: no allocation per tick)
    pub tick_buf: Vec<Cid>,
    /// a scratch buffer of dispatch()
    pub scratch: Vec<Cid>,
    /// the signals a dispatch found delivered (kept: no allocation)
    pub sig_buf: Vec<i32>,
    /// tick() found no PHP-buffered stream data just before the blocking poll it leads to
    pub buffers_checked: bool,
    pub microtasks: VecDeque<Microtask>,
    /// the positional arguments of the queued PHP microtasks, back to back in queue order
    pub microtask_args: VecDeque<Zv>,
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
    /// enabled callbacks, referenced or not (the embedding API's idle check)
    pub enabled_n: usize,
    pub callback_fiber: Zv,
    /// the error fibers' function (a Closure over the driver), made on first use
    pub error_closure: Zv,
    /// the fiber that last suspended with the internal marker
    pub yielded: *mut sys::zend_object,
    /// the suspensions of fibers (weak keys), and `{main}`'s (cleared when it is freed)
    pub suspensions: WeakTable,
    pub main_suspension: *mut sys::zend_object,
    /// the handler and the exception of an error-handler fiber about to start
    pub error_stash: Option<(Zv, Zv, Desc)>,
    /// native callbacks released after their invocation (cancelled while running)
    pub deferred_release: Vec<Cb>,
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
            callbacks: Records::default(),
            enable_queue: Vec::new(),
            enable_queue_len: 0,
            defer_queue: Vec::new(),
            defer_queue_len: 0,
            spare_queue: Vec::new(),
            callback_queue: VecDeque::new(),
            tick_buf: Vec::new(),
            scratch: Vec::new(),
            sig_buf: Vec::new(),
            buffers_checked: false,
            microtasks: VecDeque::new(),
            microtask_args: VecDeque::new(),
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
            enabled_n: 0,
            callback_fiber: Zv::undef(),
            error_closure: Zv::undef(),
            yielded: null_mut(),
            suspensions: WeakTable::new(),
            main_suspension: null_mut(),
            error_stash: None,
            deferred_release: Vec::new(),
            reactor: backend::Reactor::new(),
            timers: timer::Heap::default(),
        }
    }

    /// Reports every zval held to the cycle collector.
    pub fn gc(&mut self, add: &mut dyn FnMut(*mut sys::zval)) {
        for r in self.callbacks.values_mut() {
            match &mut r.cb {
                Cb::Php { closure, .. } => add(&mut *closure.0),
                Cb::Native { held, .. } => add(&mut *held.0),
                Cb::Taken => {}
            }
            if let Watch::Stream { stream, .. } = &mut r.watch {
                add(&mut *stream.0);
            }
            if !r.view.is_null() {
                let mut z = unsafe { zval::obj_owned(r.view) };
                add(&mut *z);
                // the buffer copies the zval: nothing to release here
                std::mem::forget(z);
            }
        }
        for a in self.microtask_args.iter_mut() {
            add(&mut *a.0);
        }
        for t in self.microtasks.iter_mut() {
            match t {
                Microtask::Php { closure, named, .. } => {
                    add(&mut *closure.0);
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
        add(&mut *self.error_closure.0);
    }

    /// A new record with the next id.
    pub fn new_record(&mut self, kind: Kind, cb: Cb, watch: Watch) -> &mut Record {
        let id = self.next_id;
        self.next_id += 1;
        self.live += 1;
        self.enabled_n += 1;
        self.callbacks.insert(Record {
            id,
            slot: 0,
            id_str: null_mut(),
            kind,
            cb,
            watch,
            enabled: true,
            invokable: false,
            referenced: true,
            queued: false,
            qgen: 0,
            active: false,
            view: null_mut(),
        })
    }

    #[inline]
    pub fn record(&mut self, id: u64) -> Option<&mut Record> {
        self.callbacks.get(id)
    }

    /// Adds to $enableQueue (or $enableDeferQueue for defers).
    pub fn queue_enable(&mut self, c: Cid) {
        let Some(r) = self.callbacks.at_id(c) else { return };
        r.queued = true;
        r.qgen = r.qgen.wrapping_add(1);
        let entry = (r.cid(), r.qgen);
        let (q, len) = if r.kind == Kind::Defer { (&mut self.defer_queue, &mut self.defer_queue_len) } else { (&mut self.enable_queue, &mut self.enable_queue_len) };
        q.push(entry);
        *len += 1;
        if q.len() > 2 * *len + 32 {
            // mostly entries of callbacks unqueued since (disable/enable churn): dropped, so
            // that the queue does not grow with them
            let recs = &mut self.callbacks;
            q.retain(|&(c, g)| recs.at_id(c).is_some_and(|r| r.queued && r.qgen == g));
        }
    }

    /// unset($this->enableQueue[$id]) / unset($this->enableDeferQueue[$id]) of `r` (counted).
    #[inline]
    pub(crate) fn unqueue(r: &mut Record, enable_len: &mut usize, defer_len: &mut usize) -> bool {
        if !r.queued {
            return false;
        }
        r.queued = false;
        if r.kind == Kind::Defer {
            *defer_len -= 1;
        } else {
            *enable_len -= 1;
        }
        true
    }

    /// unset($this->enableQueue[$id]) / unset($this->enableDeferQueue[$id])
    pub fn unqueue_enable(&mut self, id: u64) -> bool {
        let Some(r) = self.callbacks.get(id) else { return false };
        Self::unqueue(r, &mut self.enable_queue_len, &mut self.defer_queue_len)
    }

    /// Drains an enable queue into `out` (cleared first): the callbacks still queued, in order
    /// (marked not queued).
    pub fn take_queue(&mut self, defer: bool, out: &mut Vec<Cid>) {
        out.clear();
        let q = if defer { &mut self.defer_queue } else { &mut self.enable_queue };
        if q.is_empty() {
            return;
        }
        // the queue's buffer and the spare one swap: neither is freed
        let mut entries = std::mem::replace(q, std::mem::take(&mut self.spare_queue));
        if defer {
            self.defer_queue_len = 0;
        } else {
            self.enable_queue_len = 0;
        }
        for &(c, g) in &entries {
            if let Some(r) = self.callbacks.at_id(c) {
                if r.queued && r.qgen == g {
                    r.queued = false;
                    out.push(c);
                }
            }
        }
        entries.clear();
        self.spare_queue = entries;
    }

    /// The enabled flag of `r`, counted (and written through to its view).
    #[inline]
    pub(crate) fn enabled(r: &mut Record, on: bool, live: &mut usize, enabled_n: &mut usize) {
        if r.enabled == on {
            return;
        }
        r.enabled = on;
        if on {
            *enabled_n += 1;
            if r.referenced {
                *live += 1;
            }
        } else {
            *enabled_n -= 1;
            if r.referenced {
                *live -= 1;
            }
        }
        view_set_bool(r.view, ViewProp::Enabled, on);
    }

    pub fn set_enabled(&mut self, id: u64, on: bool) {
        if let Some(r) = self.callbacks.get(id) {
            Self::enabled(r, on, &mut self.live, &mut self.enabled_n);
        }
    }

    pub fn set_referenced(&mut self, id: u64, on: bool) {
        if let Some(r) = self.callbacks.get(id) {
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
        if let Some(r) = self.callbacks.get(id) {
            r.invokable = on;
            view_set_bool(r.view, ViewProp::Invokable, on);
        }
    }

    /// unset($this->callbacks[$id]) (after disable()).
    #[inline]
    pub fn remove(&mut self, id: u64) -> Option<Record> {
        let r = self.callbacks.get(id)?;
        if r.enabled {
            self.enabled_n -= 1;
            if r.referenced {
                self.live -= 1;
            }
        }
        let slot = r.slot;
        self.timers.remove(slot);
        self.callbacks.remove(id)
    }

    /// enqueueCallback()
    #[inline]
    pub fn enqueue(&mut self, c: Cid) {
        self.callback_queue.push_back(c);
        self.idle = false;
    }

    /// The ids of all callbacks, in creation order (getIdentifiers()).
    pub fn ids(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.callbacks.values().map(|r| r.id).collect();
        v.sort_unstable();
        v
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // release the records (and their native data) before the reactor goes
        self.callbacks.clear();
        self.microtasks.clear();
        self.microtask_args.clear();
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
