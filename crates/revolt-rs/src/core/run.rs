//! The loop: AbstractDriver's tick(), invokeCallbacks(), the callback fiber, microtasks, errors
//! and interrupts, and the callback operations (defer/delay/.../cancel) with Revolt's semantics.
//!
//! A `D` is a driver object; `d.c()` is its Core. A `&mut Core` is never kept across a call into
//! PHP (which may re-enter the driver): it is fetched again after each call.

use super::{backend, clock, Cb, Cid, Core, Desc, Interrupt, Kind, Microtask, Phase};
use crate::classes;
use crate::zend::{self, call, fiber, hash, stream, sys, zstr, zval, zval::Zv};
use std::ptr::null_mut;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct D(pub *mut sys::zend_object);

impl D {
    /// The driver's Core. Never hold the reference across a call into PHP.
    #[inline(always)]
    #[allow(clippy::mut_from_ref)]
    pub fn c(self) -> &'static mut Core {
        unsafe { classes::driver::core(self.0) }
    }
}

/// How the loop gave control back.
pub enum Step {
    /// no enabled referenced callback is left (or stop() was called)
    Done,
    /// `{main}` must take this (Revolt: the loop fiber suspended with an interrupt closure)
    Interrupt(Interrupt),
    /// an exception is pending (thrown by a driver hook, a fiber switch, exit()...)
    Exception,
}

// ---------------------------------------------------------------- callback operations

/// $this->now(): the clock, or a userland driver's now().
pub fn now(d: D) -> Option<f64> {
    match d.c().hooks.now {
        None => Some(clock::now()),
        Some(f) => unsafe {
            let mut r = call::call(&call::Callee { func: f, obj: d.0, scope: (*d.0).ce }, &mut [], null_mut());
            if call::has_exception() {
                return None;
            }
            let z = zval::deref(r.as_ptr());
            Some(match zval::ty(z) {
                zval::DOUBLE => (*z).value.dval,
                zval::LONG => (*z).value.lval as f64,
                _ => 0.0,
            })
        },
    }
}

/// A PHP closure as a callback.
pub unsafe fn php_cb(closure: *mut sys::zend_object) -> Cb {
    let callee = call::callee_of(closure).expect("a Closure is callable");
    Cb::Php { closure: Zv(zval::obj_ref(closure)), callee }
}

pub fn defer(d: D, cb: Cb) -> u64 {
    let c = d.c();
    let r = c.new_record(Kind::Defer, cb).cid();
    c.queue_enable(r);
    r.id
}

/// delay()/repeat(): None when now() threw.
pub fn timer(d: D, interval: f64, cb: Cb, repeat: bool) -> Option<u64> {
    let now = now(d)?;
    let c = d.c();
    let r = c.new_record(if repeat { Kind::Repeat } else { Kind::Delay }, cb);
    r.interval = interval;
    r.expiration = now + interval;
    let r = r.cid();
    c.queue_enable(r);
    Some(r.id)
}

pub fn stream(d: D, stream: Zv, cb: Cb, writable: bool) -> u64 {
    let c = d.c();
    let r = c.new_record(if writable { Kind::Writable } else { Kind::Readable }, cb);
    r.stream = stream;
    let r = r.cid();
    c.queue_enable(r);
    r.id
}

/// onReadable/onWritable on a raw OS handle (embedding API): no PHP stream.
pub fn stream_handle(d: D, h: stream::Handle, cb: Cb, writable: bool) -> u64 {
    let c = d.c();
    let r = c.new_record(if writable { Kind::Writable } else { Kind::Readable }, cb);
    r.handle = Some(h);
    r.stream = Zv::null();
    let r = r.cid();
    c.queue_enable(r);
    r.id
}

pub fn signal(d: D, sig: i32, cb: Cb) -> u64 {
    let c = d.c();
    let r = c.new_record(Kind::Signal, cb);
    r.signal = sig;
    let r = r.cid();
    c.queue_enable(r);
    r.id
}

/// enable(): false when the id is invalid (the caller throws InvalidCallbackError); None when
/// a userland now() threw.
pub fn enable(d: D, id: u64) -> Option<bool> {
    let c = d.c();
    let Some(r) = c.record(id) else { return Some(false) };
    if r.enabled {
        return Some(true);
    }
    let (kind, cid) = (r.kind, r.cid());
    c.set_enabled(id, true);
    if kind == Kind::Delay || kind == Kind::Repeat {
        let now = now(d)?;
        let c = d.c();
        if let Some(r) = c.callbacks.at_id(cid) {
            r.expiration = now + r.interval;
            let (v, e) = (r.view, r.expiration);
            if !v.is_null() {
                unsafe { classes::callbacks::view_write(v, super::ViewProp::Expiration, zval::double(e)) };
            }
        }
    }
    d.c().queue_enable(cid);
    Some(true)
}

/// disable(): false when deactivate() threw.
pub fn disable(d: D, id: u64) -> bool {
    let c = d.c();
    let Some(r) = c.callbacks.get(id) else { return true };
    if !r.enabled {
        return true;
    }
    Core::enabled(r, false, &mut c.live, &mut c.enabled_n);
    r.invokable = false;
    super::view_set_bool(r.view, super::ViewProp::Invokable, false);
    if Core::unqueue(r, &mut c.enable_queue_len, &mut c.defer_queue_len) || r.kind == Kind::Defer {
        // only queued to be enabled, or a defer
        return true;
    }
    if c.hooks.deactivate.is_some() {
        return deactivate(d, id);
    }
    let plan = deactivation(r);
    deactivate_apply(c, plan);
    true
}

/// cancel(): $this->disable() (a userland override is called), then forgotten.
pub fn cancel(d: D, id: u64) -> bool {
    let c = d.c();
    if c.hooks.disable.is_none() && c.hooks.deactivate.is_none() {
        if let Some(cid) = c.record(id).map(|r| r.cid()) {
            cancel_at(c, cid);
        }
        return true;
    }
    if !v_disable(d, id) {
        return false;
    }
    let c = d.c();
    if let Some(mut r) = c.remove(id) {
        if r.active && c.hooks.deactivate.is_none() {
            // a userland disable() that did not deactivate: keep the reactor consistent
            deactivate_record(c, &mut r);
        }
        if let Cb::Native { .. } = r.cb {
            if c.in_call == id {
                c.deferred_release.push(std::mem::replace(&mut r.cb, Cb::Taken));
            }
        }
    }
    true
}

/// cancel() without PHP hooks (disable() and the removal in one pass): no PHP code runs.
fn cancel_at(c: &mut Core, cid: Cid) {
    let Some(r) = c.callbacks.at_id(cid) else { return };
    if r.enabled {
        Core::enabled(r, false, &mut c.live, &mut c.enabled_n);
        r.invokable = false;
        super::view_set_bool(r.view, super::ViewProp::Invokable, false);
        Core::unqueue(r, &mut c.enable_queue_len, &mut c.defer_queue_len);
    }
    let plan = deactivation(r);
    if matches!(r.cb, Cb::Native { .. }) && c.in_call == cid.id {
        // released after its invocation returns
        c.deferred_release.push(std::mem::replace(&mut r.cb, Cb::Taken));
    }
    let r = c.callbacks.remove(cid.id);
    deactivate_apply(c, plan);
    // dropped last: releasing its closure may run PHP code
    drop(r);
}

pub fn reference(d: D, id: u64) -> bool {
    let c = d.c();
    if c.record(id).is_none() {
        return false;
    }
    c.set_referenced(id, true);
    true
}

pub fn unreference(d: D, id: u64) {
    d.c().set_referenced(id, false);
}

/// The callback's id string (a new reference), or null.
fn id_zstr(d: D, id: u64) -> *mut sys::zend_string {
    match d.c().record(id) {
        Some(r) => {
            let s = r.id_zstr();
            unsafe { zstr::addref(s) };
            s
        }
        None => unsafe { zstr::new(super::id_bytes(id).as_bytes()) },
    }
}

/// Calls a userland override `f($id)` of cancel/disable/enable; false when it threw.
fn call_override(d: D, f: *mut sys::zend_function, id: u64) -> bool {
    unsafe {
        let mut a = zval::str_owned(id_zstr(d, id));
        let _ = call::call(&call::Callee { func: f, obj: d.0, scope: (*d.0).ce }, std::slice::from_mut(&mut a), null_mut());
        zval::dtor(&mut a);
        !call::has_exception()
    }
}

/// $this->cancel($id) as Revolt's own code calls it (a userland override included).
pub fn v_cancel(d: D, id: u64) -> bool {
    match d.c().hooks.cancel {
        Some(f) => call_override(d, f, id),
        None => cancel(d, id),
    }
}

pub fn v_disable(d: D, id: u64) -> bool {
    match d.c().hooks.disable {
        Some(f) => call_override(d, f, id),
        None => disable(d, id),
    }
}

pub fn v_enable(d: D, id: u64) -> bool {
    match d.c().hooks.enable {
        Some(f) => call_override(d, f, id),
        None => match enable(d, id) {
            Some(true) => true,
            Some(false) => unsafe {
                classes::errors::throw_invalid_identifier(id_zstr(d, id));
                false
            },
            None => false,
        },
    }
}

// ---------------------------------------------------------------- activation (the reactor)

/// activate($callbacks): false when it threw (the callbacks not activated go back to the queue).
fn activate(d: D, ids: &[Cid]) -> bool {
    let c = d.c();
    if let Some(f) = c.hooks.activate {
        unsafe {
            let arr = hash::new(ids.len() as u32);
            for &cid in ids {
                let v = classes::callbacks::view(d, cid.id);
                if v.is_null() {
                    continue;
                }
                let r = d.c().callbacks.at_id(cid).unwrap();
                hash::set_zstr(arr, r.id_zstr(), zval::obj_ref(v));
            }
            let mut a = zval::arr_owned(arr);
            let _ = call::call(&call::Callee { func: f, obj: d.0, scope: (*d.0).ce }, std::slice::from_mut(&mut a), null_mut());
            zval::dtor(&mut a);
            if call::has_exception() {
                requeue(d.c(), ids);
                return false;
            }
            let c = d.c();
            for &cid in ids {
                if let Some(r) = c.callbacks.at_id(cid) {
                    r.active = true;
                }
            }
        }
        return true;
    }
    for (i, &cid) in ids.iter().enumerate() {
        if let Err(e) = activate_native(c, cid) {
            requeue(c, &ids[i..]);
            unsafe { e.throw() };
            return false;
        }
    }
    true
}

fn requeue(c: &mut Core, ids: &[Cid]) {
    for &cid in ids {
        if c.callbacks.at_id(cid).is_some_and(|r| r.enabled && !r.queued) {
            c.queue_enable(cid);
        }
    }
}

pub enum ActivateError {
    Stream(u64),
    Signal(i32, String),
}

/// activate() of one callback (a native driver's activate() called from PHP).
pub fn activate_one(d: D, id: u64) -> Result<(), ActivateError> {
    let c = d.c();
    match c.record(id).map(|r| r.cid()) {
        Some(cid) => activate_native(c, cid),
        None => Ok(()),
    }
}

/// deactivate() of one callback (a native driver's deactivate() called from PHP).
pub fn deactivate_one(d: D, id: u64) {
    deactivate_id(d.c(), id);
}

/// Deactivates the record `id` in place.
fn deactivate_id(c: &mut Core, id: u64) {
    if let Some(r) = c.callbacks.get(id) {
        let d = deactivation(r);
        deactivate_apply(c, d);
    }
}

/// dispatch() of a native driver called from PHP.
pub fn dispatch_one(d: D, blocking: bool) {
    dispatch_native(d.c(), blocking);
    flush_sources(d);
}

/// The source groups the last poll gave events to are flushed (embedding API): each resumes
/// what its sources' waiters took, outside the callback fiber (it calls no PHP code). An
/// exception it leaves is handled as a native callback's.
pub fn flush_sources(d: D) {
    while let Some((flush, data)) = d.c().reactor.next_flush() {
        unsafe { flush(data) };
        if call::has_exception() {
            if unsafe { call::exception_is_unwind() } {
                return;
            }
            let e = Zv(unsafe { zval::obj_owned(call::take_exception()) });
            error(d, Desc::Static("native callback"), e);
        }
    }
}

impl ActivateError {
    pub unsafe fn raise(&self) {
        self.throw()
    }

    unsafe fn throw(&self) {
        match self {
            ActivateError::Stream(id) => {
                let s = String::from_utf8_lossy(super::id_bytes(*id).as_bytes()).into_owned();
                call::throw(sys::zend_ce_type_error, &format!("Invalid stream for callback {s}: not a valid stream resource (closed, or not a stream)"));
            }
            ActivateError::Signal(errno, msg) => {
                call::throw(classes::ce().unsupported_feature, &format!("Failed to register signal handler; Errno: {errno}; {msg}"));
            }
        }
    }
}

fn activate_native(c: &mut Core, cid: Cid) -> Result<(), ActivateError> {
    let id = cid.id;
    let Some(r) = c.callbacks.at_id(cid) else { return Ok(()) };
    if r.active {
        return Ok(());
    }
    match r.kind {
        Kind::Readable | Kind::Writable => {
            let h = match r.handle {
                Some(h) if zval::is_null(&r.stream.0) => h,
                _ => {
                    let h = unsafe { stream::handle(&mut *r.stream.0) }.ok_or(ActivateError::Stream(id))?;
                    r.handle = Some(h);
                    h
                }
            };
            let dir = if r.kind == Kind::Readable { backend::READ } else { backend::WRITE };
            r.active = true;
            c.reactor.add(h.fd, h.socket, h.res, cid, dir);
            if dir == backend::READ && !zval::is_null(&r.stream.0) {
                let res = unsafe { stream::resource(&mut *r.stream.0) };
                r.reader_idx = c.reactor.php_readers.len() as u32;
                c.reactor.php_readers.push((cid, res));
            }
        }
        Kind::Delay | Kind::Repeat => {
            r.active = true;
            c.timers.insert(cid, r.expiration);
        }
        Kind::Signal => {
            let sig = r.signal;
            c.signals.add(sig, cid).map_err(|(e, m)| ActivateError::Signal(e, m))?;
            c.callbacks.at(cid.slot).active = true;
            c.reactor.set_wake(c.signals.wake_fd());
        }
        Kind::Defer => {}
    }
    Ok(())
}

/// deactivate($callback): false when a userland deactivate() threw.
fn deactivate(d: D, id: u64) -> bool {
    let c = d.c();
    if let Some(f) = c.hooks.deactivate {
        unsafe {
            let v = classes::callbacks::view(d, id);
            if v.is_null() {
                return true;
            }
            let mut a = zval::obj_ref(v);
            let _ = call::call(&call::Callee { func: f, obj: d.0, scope: (*d.0).ce }, std::slice::from_mut(&mut a), null_mut());
            zval::dtor(&mut a);
            if let Some(r) = d.c().record(id) {
                r.active = false;
            }
            return !call::has_exception();
        }
    }
    deactivate_id(c, id);
    true
}

/// What taking a record out of the reactor takes (computed from the record, then applied to the
/// reactor: no borrow of the record meanwhile).
enum Deactivation {
    None,
    Fd { fd: i64, id: u64, dir: u8, php_reader: u32 },
    Timer { slot: u32 },
    Signal { sig: i32, id: u64 },
}

/// Marks `r` inactive; what remains to be done in the reactor.
fn deactivation(r: &mut super::Record) -> Deactivation {
    if !r.active {
        return Deactivation::None;
    }
    r.active = false;
    match r.kind {
        Kind::Readable | Kind::Writable => match r.handle {
            Some(h) => Deactivation::Fd {
                fd: h.fd,
                id: r.id,
                dir: if r.kind == Kind::Readable { backend::READ } else { backend::WRITE },
                php_reader: std::mem::replace(&mut r.reader_idx, u32::MAX),
            },
            None => Deactivation::None,
        },
        Kind::Delay | Kind::Repeat => Deactivation::Timer { slot: r.slot },
        Kind::Signal => Deactivation::Signal { sig: r.signal, id: r.id },
        _ => Deactivation::None,
    }
}

fn deactivate_apply(c: &mut Core, d: Deactivation) {
    match d {
        Deactivation::None => {}
        Deactivation::Fd { fd, id, dir, php_reader } => {
            c.reactor.del(fd, id, dir);
            if php_reader != u32::MAX {
                let rs = &mut c.reactor.php_readers;
                rs.swap_remove(php_reader as usize);
                if let Some(&(moved, _)) = rs.get(php_reader as usize) {
                    if let Some(m) = c.callbacks.at_id(moved) {
                        m.reader_idx = php_reader;
                    }
                }
            }
        }
        Deactivation::Timer { slot } => c.timers.remove(slot),
        Deactivation::Signal { sig, id } => {
            c.signals.remove(sig, id);
            c.reactor.set_wake(c.signals.wake_fd());
        }
    }
}

/// Takes `r` (removed from the table, and from the timer heap then) out of the reactor.
fn deactivate_record(c: &mut Core, r: &mut super::Record) {
    let d = deactivation(r);
    deactivate_apply(c, d);
}

// ---------------------------------------------------------------- dispatch

/// dispatch($blocking): false when it threw.
fn dispatch(d: D, blocking: bool) -> bool {
    let c = d.c();
    if let Some(f) = c.hooks.dispatch {
        unsafe {
            let mut a = zval::bool(blocking);
            let _ = call::call(&call::Callee { func: f, obj: d.0, scope: (*d.0).ce }, std::slice::from_mut(&mut a), null_mut());
            return !call::has_exception();
        }
    }
    dispatch_native(c, blocking);
    flush_sources(d);
    true
}

fn dispatch_native(c: &mut Core, mut blocking: bool) {
    if !c.signals.by_signal.is_empty() {
        let mut delivered = std::mem::take(&mut c.sig_buf);
        c.signals.take_delivered(false, &mut delivered);
        if !delivered.is_empty() {
            enqueue_signals(c, &delivered);
            blocking = false;
        }
        c.sig_buf = delivered;
    }
    if blocking && !std::mem::take(&mut c.buffers_checked) {
        let mut buffered = std::mem::take(&mut c.scratch);
        buffered_readers(c, &mut buffered);
        if !buffered.is_empty() {
            buffered.sort_unstable_by_key(|r| r.id);
            for &cid in &buffered {
                c.enqueue(cid);
            }
            blocking = false;
        }
        c.scratch = buffered;
    }
    c.buffers_checked = false;
    let deadline = if blocking { c.timers.peek() } else { None };
    let timeout = if blocking { deadline.map(|e| (e - clock::now()).max(0.0)) } else { Some(0.0) };
    let mut ready = std::mem::take(&mut c.ready_buf);
    ready.clear();
    // the deadline on the loop clock (a PHP now() override's timers are on its own clock)
    let deadline = deadline.filter(|_| c.hooks.now.is_none());
    let woke = c.reactor.poll(timeout, deadline, &mut ready);
    for &(fd, rd, wr) in &ready {
        let Some(e) = c.reactor.fds.get(&fd) else { continue };
        if rd {
            c.callback_queue.extend(e.readers.iter().copied());
        }
        if wr {
            c.callback_queue.extend(e.writers.iter().copied());
        }
        if rd || wr {
            c.idle = false;
        }
    }
    c.ready_buf = ready;
    if c.reactor.flush_pending() {
        // waiters of sources resume: the next tick polls first, as for ready callbacks
        c.idle = false;
    }
    if woke {
        let mut delivered = std::mem::take(&mut c.sig_buf);
        c.signals.take_delivered(true, &mut delivered);
        enqueue_signals(c, &delivered);
        c.sig_buf = delivered;
    }
    if c.timers.len() > 0 {
        let now = clock::now();
        while let Some(cid) = c.timers.pop_expired(now) {
            c.enqueue(cid);
        }
    }
}

/// The readable callbacks of streams with data in PHP's read buffers (stream_select reports
/// such streams readable at once), into `out` (cleared first). Raw handles (embedders' sockets)
/// have no PHP buffers.
fn buffered_readers(c: &mut Core, out: &mut Vec<Cid>) {
    out.clear();
    // SAFETY: the records hold their resources while they are listed
    out.extend(c.reactor.php_readers.iter().filter(|&&(_, res)| unsafe { stream::res_buffered(res) }).map(|&(cid, _)| cid));
}

/// Whether some readable callback's stream has data in PHP's read buffer.
fn any_buffered(c: &mut Core) -> bool {
    c.reactor.php_readers.iter().any(|&(_, res)| unsafe { stream::res_buffered(res) })
}

fn enqueue_signals(c: &mut Core, sigs: &[i32]) {
    for s in sigs {
        if let Some(ids) = c.signals.by_signal.get(s) {
            if !ids.is_empty() {
                c.callback_queue.extend(ids.iter().copied());
                c.idle = false;
            }
        }
    }
}

// ---------------------------------------------------------------- the loop

/// tick(): activation and the first half; returns the next phase, or None when it threw.
fn tick(d: D, previous_idle: bool) -> Option<Phase> {
    // the tick's buffer (taken: activate() may call PHP, which may tick a nested loop)
    let mut ids = std::mem::take(&mut d.c().tick_buf);
    d.c().take_queue(false, &mut ids);
    if !ids.is_empty() && !activate(d, &ids) {
        d.c().tick_buf = ids;
        return None;
    }
    let c = d.c();
    for &cid in &ids {
        if let Some(r) = c.callbacks.at_id(cid) {
            r.invokable = true;
            super::view_set_bool(r.view, super::ViewProp::Invokable, true);
        }
    }
    c.take_queue(true, &mut ids);
    for &cid in &ids {
        if let Some(r) = c.callbacks.at_id(cid) {
            r.invokable = true;
            super::view_set_bool(r.view, super::ViewProp::Invokable, true);
        }
        c.enqueue(cid);
    }
    c.tick_buf = ids;
    // Revolt polls without blocking after a tick that ran callbacks, then blocks in the next
    // tick if that poll found nothing. With nothing queued now, nothing can run between those
    // two polls, so one blocking poll gives the same result (it returns at once for what the
    // first would have found) without the extra system call. Not with PHP overrides of
    // activate()/dispatch()/now() (they see every tick), nor with buffered stream data (the
    // blocking poll reports it before the ready fds, the non-blocking one after them).
    let h = &c.hooks;
    let quiet = !previous_idle
        && c.callback_queue.is_empty()
        && c.microtasks.is_empty()
        && h.activate.is_none()
        && h.dispatch.is_none()
        && h.now.is_none()
        && !any_buffered(c);
    // the blocking poll this leads to (nothing runs before it) need not look again
    c.buffers_checked = quiet;
    let blocking = (previous_idle || quiet) && !c.stopped && c.live > 0;
    if blocking {
        c.tick_blocking = true;
        return Some(Phase::TickInvoke);
    }
    if !dispatch(d, false) {
        return None;
    }
    Some(Phase::AfterTick)
}

/// Runs the loop until it is empty, stopped, or `{main}` must be interrupted (Revolt: resumes
/// the loop fiber). An exception ends it (Revolt: the loop fiber terminates).
fn resume_loop(d: D) -> Step {
    let s = resume_loop_inner(d);
    if let Step::Exception = s {
        d.c().phase = Phase::Idle;
    }
    s
}

fn resume_loop_inner(d: D) -> Step {
    let c = d.c();
    if c.phase == Phase::Idle {
        c.stopped = false;
        c.phase = Phase::First;
    }
    loop {
        let c = d.c();
        match c.phase {
            Phase::Idle => return Step::Done,
            Phase::First | Phase::AfterTick => {
                if let Some(s) = invoke_callbacks(d) {
                    return s;
                }
                // an interrupt set outside the callback fiber (a source flush during dispatch)
                // is taken as one set by a callback: the loop goes on with invokeCallbacks()
                // when it resumes (what was queued meanwhile runs before the next isEmpty())
                if let Some(i) = d.c().interrupt.take() {
                    return Step::Interrupt(i);
                }
                d.c().phase = Phase::Top;
            }
            Phase::TickInvoke => {
                if let Some(s) = invoke_callbacks(d) {
                    return s;
                }
                let c = d.c();
                let blocking = c.tick_blocking && c.enable_queue_len == 0 && c.defer_queue_len == 0;
                c.tick_blocking = false;
                c.phase = Phase::AfterTick;
                if !dispatch(d, blocking) {
                    return Step::Exception;
                }
            }
            Phase::Top => {
                if c.stopped {
                    c.phase = Phase::Idle;
                    return Step::Done;
                }
                if let Some(i) = c.interrupt.take() {
                    return Step::Interrupt(i);
                }
                if c.live == 0 {
                    c.phase = Phase::Idle;
                    return Step::Done;
                }
                let previous = c.idle;
                c.idle = true;
                match tick(d, previous) {
                    Some(p) => d.c().phase = p,
                    None => {
                        // Revolt: the exception ends the loop fiber
                        d.c().phase = Phase::Idle;
                        return Step::Exception;
                    }
                }
            }
        }
    }
}

/// Revolt's runCallback: runs the loop, then collects cycles and runs it again while that
/// freed something and the loop was not stopped.
pub fn run_main(d: D) -> Step {
    unsafe { (*d.0).gc.refcount += 1 };
    let frame = unsafe { (*zend::eg()).current_execute_data };
    let step = loop {
        let c = d.c();
        c.in_loop = frame;
        let s = resume_loop(d);
        d.c().in_loop = null_mut();
        match s {
            Step::Done => {}
            other => break other,
        }
        if zend::gc_collect_cycles() == 0 || d.c().stopped {
            break Step::Done;
        }
        if call::has_exception() {
            break Step::Exception;
        }
    };
    unsafe { zval::obj_unref_pair(d.0) };
    step
}

/// Whether the loop is executing on the current call stack (Revolt: the loop fiber is running).
/// A loop left by a fatal error (bailout) is found stale and reset.
pub fn loop_running(d: D) -> bool {
    let c = d.c();
    if c.in_loop.is_null() {
        return false;
    }
    let mut f = unsafe { (*zend::eg()).current_execute_data };
    while !f.is_null() {
        if f == c.in_loop {
            return true;
        }
        f = unsafe { (*f).prev_execute_data };
    }
    // the frames that ran the loop are gone (a bailout): start over
    c.in_loop = null_mut();
    c.phase = Phase::Idle;
    false
}

/// invokeCallbacks(): runs the callback fiber while microtasks or callbacks are queued; Some
/// when `{main}` must be interrupted or an exception is pending.
fn invoke_callbacks(d: D) -> Option<Step> {
    loop {
        let c = d.c();
        if c.microtasks.is_empty() && c.callback_queue.is_empty() {
            return None;
        }
        let mut f = c.callback_fiber.obj();
        let reusable = !f.is_null() && unsafe { fiber::status(f) == fiber::STATUS_SUSPENDED || fiber::status(f) == fiber::STATUS_INIT } && !unsafe { fiber::is_destroyed(f) };
        if !reusable {
            f = new_fiber(d, unsafe { CALLBACK_FIBER_FN });
            if f.is_null() {
                return Some(Step::Exception);
            }
            d.c().callback_fiber.set(unsafe { zval::obj_owned(f) });
        }
        d.c().yielded = null_mut();
        unsafe {
            (*f).gc.refcount += 1;
            let r = if fiber::status(f) == fiber::STATUS_INIT {
                fiber::start(f)
            } else {
                let mut n = zval::null();
                fiber::resume(f, &mut n)
            };
            drop(r);
            let c = d.c();
            if c.yielded != f && c.callback_fiber.obj() == f {
                // the callback suspended (its Suspension owns the fiber now) or the fiber ended
                c.callback_fiber.set(zval::undef());
            }
            zval::obj_unref_pair(f);
        }
        if call::has_exception() {
            return Some(Step::Exception);
        }
        if let Some(i) = d.c().interrupt.take() {
            return Some(Step::Interrupt(i));
        }
    }
}

static mut CALLBACK_FIBER_FN: *mut sys::zend_function = null_mut();
static mut ERROR_FIBER_FN: *mut sys::zend_function = null_mut();

pub unsafe fn minit(scope: *mut sys::zend_class_entry) {
    CALLBACK_FIBER_FN = fiber::internal_function("{closure}", crate::zh!(callback_fiber_body), scope);
    ERROR_FIBER_FN = fiber::internal_function("{closure}", crate::zh!(error_fiber_body), scope);
}

/// A new Fiber running `func` with `$this` = the driver; null with an exception.
fn new_fiber(d: D, func: *mut sys::zend_function) -> *mut sys::zend_object {
    unsafe {
        let mut cl = fiber::closure(func, d.0);
        fiber::new_tracked(&mut cl)
    }
}

/// Suspends the callback fiber with the internal marker; false when it must end (destroyed,
/// or an exception).
fn suspend_marker(d: D) -> bool {
    d.c().yielded = zend::active_fiber();
    unsafe {
        let mut n = zval::null();
        drop(fiber::suspend(&mut n));
    }
    !call::has_exception()
}

/// The callback fiber's function (createCallbackFiber()).
unsafe extern "C" fn callback_fiber_body(ex: call::Ex, _rv: *mut sys::zval) {
    let d = D(call::this(ex));
    loop {
        if !invoke_microtasks(d) {
            return;
        }
        loop {
            let Some(cid) = d.c().callback_queue.pop_front() else { break };
            if !invoke_callback(d, cid) {
                return;
            }
            if d.c().interrupt.is_some() && !suspend_marker(d) {
                return;
            }
            if !invoke_microtasks(d) {
                return;
            }
        }
        if !suspend_marker(d) {
            return;
        }
    }
}

/// One callback of the queue; false when the fiber must end.
unsafe fn invoke_callback(d: D, cid: Cid) -> bool {
    let id = cid.id;
    let c = d.c();
    // no PHP code runs during the cancel() of a callback fired once (a native driver): what the
    // call needs moves out of the record instead of being copied
    let native_cancel = c.hooks.cancel.is_none() && c.hooks.disable.is_none() && c.hooks.deactivate.is_none();
    let Some(r) = c.callbacks.at_id(cid) else { return true };
    if !r.invokable {
        return true;
    }
    let kind = r.kind;
    let fired_once = matches!(kind, Kind::Defer | Kind::Delay);
    let native = match &mut r.cb {
        Cb::Php { .. } => None,
        Cb::Native { f, data, release, held } => {
            if fired_once {
                // invoked after its cancel(): its data is released after the call
                let cb = Cb::Native { f: *f, data: *data, release: *release, held: Zv(held.take()) };
                std::mem::forget(std::mem::replace(&mut r.cb, Cb::Taken));
                Some(cb)
            } else {
                Some(Cb::Native { f: *f, data: *data, release: None, held: Zv::undef() })
            }
        }
        Cb::Taken => return true,
    };
    let fiber = zend::active_fiber();
    let is_php = matches!(r.cb, Cb::Php { .. });
    let id_str = if is_php { r.id_zstr() } else { null_mut() };
    let callee = if fired_once && native_cancel && is_php {
        let Cb::Php { closure, callee } = &mut r.cb else { unreachable!() };
        let (closure, callee) = (closure.take(), *callee);
        // the record keeps nothing to release: its closure and id string are the frame's now
        std::mem::forget(std::mem::replace(&mut r.cb, Cb::Taken));
        let id_str = zval::str_owned(r.take_id_zstr());
        fiber::inflight::hold(fiber, [closure, id_str]);
        Some(callee)
    } else if let Cb::Php { closure, callee } = &r.cb {
        // what the call needs, kept even if the callback is cancelled before it runs; held where
        // the cycle collector sees it (the callback fiber's frames, in Revolt)
        let third = match kind {
            Kind::Readable | Kind::Writable => Some(zval::copy(&*r.stream.0)),
            Kind::Signal => Some(zval::long(r.signal as i64)),
            _ => None,
        };
        let closure = zval::copy(&*closure.0);
        let id_str = zval::str_ref(id_str);
        fiber::inflight::hold(fiber, [closure, id_str].into_iter().chain(third));
        Some(*callee)
    } else {
        None
    };
    let ok = match kind {
        Kind::Defer | Kind::Delay if native_cancel && d.c().hooks.cancel.is_none() => {
            cancel_at(d.c(), cid);
            true
        }
        Kind::Defer | Kind::Delay => v_cancel(d, id),
        // disable and re-enable, so it does not run again in the same tick (amphp/amp#131)
        Kind::Repeat => v_disable(d, id) && v_enable(d, id),
        _ => true,
    };
    if let Some(callee) = callee {
        if ok {
            let (p, n) = fiber::inflight::params(fiber);
            if !p.is_null() {
                let params = std::slice::from_raw_parts_mut(p.add(1), n - 1);
                let rv = call::call(&callee, params, null_mut());
                let mut held = fiber::inflight::take(fiber);
                let closure = Zv(zval::take(&mut held[0]));
                for z in held[1..].iter_mut() {
                    zval::dtor(z);
                }
                let mut rv = rv;
                if call::has_exception() {
                    if call::exception_is_unwind() {
                        return false;
                    }
                    let e = Zv(zval::obj_owned(call::take_exception()));
                    error(d, Desc::Closure(closure), e);
                } else if !zval::is_null(rv.as_ptr()) && !zval::is_undef(rv.as_ptr()) {
                    let e = classes::errors::non_null_return(id, closure.obj());
                    if !e.is_null() {
                        error(d, Desc::Closure(closure), Zv(zval::obj_owned(e)));
                    }
                }
            }
        } else {
            for z in fiber::inflight::take(fiber).iter_mut() {
                zval::dtor(z);
            }
            return false;
        }
    } else if let Some(cb) = native {
        if !ok {
            return false;
        }
        if let Cb::Native { f, data, .. } = &cb {
            d.c().in_call = id;
            f(*data, id);
            d.c().in_call = 0;
            d.c().deferred_release.clear();
        }
        drop(cb);
        if call::has_exception() {
            if call::exception_is_unwind() {
                return false;
            }
            let e = Zv(zval::obj_owned(call::take_exception()));
            error(d, Desc::Static("native callback"), e);
        }
    }
    classes::fiber_local::clear();
    !call::has_exception()
}

/// invokeMicrotasks(); false when the fiber must end.
fn invoke_microtasks(d: D) -> bool {
    loop {
        let Some(task) = d.c().microtasks.pop_front() else { return true };
        unsafe {
            let fiber = zend::active_fiber();
            match task {
                Microtask::Php { closure, nargs, named } => {
                    let c = d.c();
                    let Some(callee) = call::callee_of(closure.obj()) else {
                        // (never: a Closure) its arguments go with it
                        c.microtask_args.drain(..nargs as usize);
                        continue;
                    };
                    // the closure, the named arguments, then the arguments: held where the
                    // cycle collector sees them while the call runs
                    let (mut closure, mut named) = (closure, named);
                    let args = c.microtask_args.drain(..nargs as usize).map(|mut a| a.take());
                    let (p, n) = fiber::inflight::hold(fiber, [closure.take(), named.take()].into_iter().chain(args));
                    let named_ht = if zval::ty(&*p.add(1)) == zval::ARRAY { (&*p.add(1)).value.arr } else { null_mut() };
                    let params = std::slice::from_raw_parts_mut(p.add(2), n - 2);
                    // the arguments go to the call (Revolt clears $args before the closure runs):
                    // the call's frame holds the only references (the engine adds one per
                    // argument, ours are given up first), so an argument the closure unsets is
                    // destroyed at once
                    let transfer = !takes_references(callee.func, params.len());
                    if transfer {
                        for a in params.iter_mut() {
                            if zval::is_refcounted(&*a) {
                                (*(**a).value.counted).gc.refcount -= 1;
                            }
                        }
                    }
                    drop(call::call(&callee, params, named_ht));
                    let mut held = fiber::inflight::take(fiber);
                    let closure = Zv(zval::take(&mut held[0]));
                    for (i, z) in held[1..].iter_mut().enumerate() {
                        if transfer && i >= 1 {
                            // given to the call, which released them
                            continue;
                        }
                        zval::dtor(z);
                    }
                    if call::has_exception() {
                        if call::exception_is_unwind() {
                            return false;
                        }
                        let e = Zv(zval::obj_owned(call::take_exception()));
                        error(d, Desc::Closure(closure), e);
                    }
                }
                Microtask::Resume { fiber: mut f, mut value, throw } => {
                    let fo = f.obj();
                    let (p, _) = fiber::inflight::hold(fiber, [f.take(), value.take()]);
                    if !fiber::is_terminated(fo) {
                        let v = &mut *p.add(1);
                        drop(if throw { fiber::throw_into(fo, v) } else { fiber::resume(fo, v) });
                    }
                    for z in fiber::inflight::take(fiber).iter_mut() {
                        zval::dtor(z);
                    }
                    if call::has_exception() {
                        if call::exception_is_unwind() {
                            return false;
                        }
                        let e = Zv(zval::obj_owned(call::take_exception()));
                        error(d, Desc::Static(if throw { "Revolt\\EventLoop\\Internal\\DriverSuspension::throw" } else { "Revolt\\EventLoop\\Internal\\DriverSuspension::resume" }), e);
                    }
                }
                Microtask::Native { f, data } => {
                    f(data, 0);
                    if call::has_exception() {
                        if call::exception_is_unwind() {
                            return false;
                        }
                        let e = Zv(zval::obj_owned(call::take_exception()));
                        error(d, Desc::Static("native microtask"), e);
                    }
                }
            }
        }
        classes::fiber_local::clear();
        if call::has_exception() {
            return false;
        }
        if d.c().interrupt.is_some() && !suspend_marker(d) {
            return false;
        }
    }
}

/// Whether a call of `f` with `n` arguments passes some by reference.
unsafe fn takes_references(f: *mut sys::zend_function, n: usize) -> bool {
    let info = (*f).common.arg_info;
    if info.is_null() {
        return false;
    }
    let declared = (*f).common.num_args as usize;
    let variadic = (*f).common.fn_flags & sys::ZEND_ACC_VARIADIC != 0;
    let by_ref = |i: usize| ((*info.add(i)).type_.type_mask >> 25) & 3 != 0;
    for i in 0..n.min(declared) {
        if by_ref(i) {
            return true;
        }
    }
    n > declared && variadic && by_ref(declared)
}

/// error(): the error handler (in a new fiber), or an interrupt that throws UncaughtThrowable in
/// `{main}` (replacing any other: hiding the exception is worse).
pub fn error(d: D, desc: Desc, ex: Zv) {
    let c = d.c();
    if zval::is_null(&c.error_handler.0) {
        c.interrupt = Some(Interrupt::Uncaught { desc, ex, handler: false });
        return;
    }
    c.error_stash = Some((c.error_handler.clone_ref(), ex, desc));
    // the closure is made once (Revolt's $this->errorCallback); each error gets a new Fiber
    if zval::is_undef(&d.c().error_closure.0) {
        let cl = unsafe { fiber::closure(ERROR_FIBER_FN, d.0) };
        d.c().error_closure = cl;
    }
    let mut cl = d.c().error_closure.clone_ref();
    let f = unsafe { fiber::new_tracked(&mut cl) };
    drop(cl);
    if f.is_null() {
        d.c().error_stash = None;
        return;
    }
    unsafe {
        drop(fiber::start(f));
        let mut z = zval::obj_owned(f);
        zval::dtor(&mut z);
    }
}

/// The error fiber's function (createErrorCallback()).
unsafe extern "C" fn error_fiber_body(ex: call::Ex, _rv: *mut sys::zval) {
    let d = D(call::this(ex));
    let Some((mut handler, mut e, _desc)) = d.c().error_stash.take() else { return };
    let Some(callee) = call::callee_of(handler.obj()) else { return };
    let fiber = zend::active_fiber();
    let (p, _) = fiber::inflight::hold(fiber, [handler.take(), e.take()]);
    drop(call::call(&callee, std::slice::from_raw_parts_mut(p.add(1), 1), null_mut()));
    let mut held = fiber::inflight::take(fiber);
    let handler = Zv(zval::take(&mut held[0]));
    for z in held[1..].iter_mut() {
        zval::dtor(z);
    }
    if call::has_exception() {
        if call::exception_is_unwind() {
            return;
        }
        let e2 = Zv(zval::obj_owned(call::take_exception()));
        d.c().interrupt = Some(Interrupt::Uncaught { desc: Desc::Closure(handler), ex: e2, handler: true });
    }
}

/// Revolt's `$lambda()` in `{main}`: the value, or None with the exception thrown.
pub fn take_interrupt(i: Interrupt) -> Option<Zv> {
    unsafe {
        match i {
            Interrupt::Value(v) => Some(v),
            Interrupt::Throw(mut e) => {
                call::rethrow(zval::take(e.as_ptr()).value.obj);
                None
            }
            Interrupt::Uncaught { desc, mut ex, handler } => {
                let eo = ex.obj();
                if call::instance_of((*eo).ce, classes::ce().uncaught_throwable) {
                    call::rethrow(zval::take(ex.as_ptr()).value.obj);
                    return None;
                }
                let u = classes::errors::uncaught(&desc, eo, handler);
                drop(ex);
                if !u.is_null() {
                    call::rethrow(u);
                }
                None
            }
        }
    }
}
