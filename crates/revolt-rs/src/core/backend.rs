//! The OS reactor: level-triggered readiness of file descriptors (Revolt's semantics: a callback
//! fires on every tick while its stream is ready), as stream_select gives it.
//!
//! - Linux: epoll without EPOLLET; macOS/BSD: kqueue without EV_CLEAR; Windows: WSAPoll for
//!   sockets, other handles (pipes, files, consoles) checked without waiting.
//! - Interest changes are batched until the next poll (like libev's fd changes): enabling and
//!   disabling a watcher between two polls costs no system call.
//! - fds the poller refuses (regular files on epoll, closed fds) are "always ready", as select
//!   reports them.
//! - fork(): a child rebuilds its poller on first use (an inherited epoll instance is shared with
//!   the parent; kqueues are not inherited).
//! - macOS: kevent64() (a poll that does not wait costs ~0.15 µs; kevent() with a zero timeout
//!   ~12 µs).
//! - macOS: timed waits go through a one-shot EVFILT_TIMER with NOTE_CRITICAL (kqueue even when
//!   no fd is watched): the system's timer coalescing otherwise delays nanosleep()/kevent()
//!   wake-ups by up to 10% of the timeout (more for background processes).
//! - Linux: timed waits go through a timerfd armed for the deadline (absolute, CLOCK_MONOTONIC:
//!   the loop clock), polled with the fds: epoll_wait()'s own timeout costs a kernel timer per
//!   call and has whole milliseconds only.
//! - Both wait timers stay armed for their deadline across polls: a wait for the same deadline
//!   (the same next timer) changes nothing, so it costs no kernel timer work.
//! - Sources (embedding API): raw fds of an embedder registered edge-triggered, outside the
//!   callback records: registered with the kernel at their first wait, kept until the embedder
//!   forgets them (before closing them), so a wait makes no control call. An event is offered to
//!   the source's armed waiter (its group's `take`, called during the poll), else kept as ready
//!   bits that the next wait of the source consumes (it then returns at once). After the poll,
//!   each group whose `take` accepted something is flushed once (one call per poll, not one per
//!   source). Linux: EPOLLET with EPOLLIN|EPOLLOUT|EPOLLRDHUP (bit 63 of the event data marks a
//!   source); kqueue: EVFILT_READ with EV_CLEAR once, EVFILT_WRITE one-shot per wait (macOS
//!   reports an edge-triggered write filter at every ACK that frees send-buffer space);
//!   Windows: an armed source is polled level-triggered with the other sockets.

use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::sync::atomic::AtomicU64;
#[cfg(unix)]
use std::sync::atomic::Ordering;

pub const READ: u8 = 1;
pub const WRITE: u8 = 2;
/// a source's error or hang-up (reported with READ)
pub const ERR: u8 = 4;

/// Marks a source in the data of its epoll event.
#[cfg(target_os = "linux")]
const SRC_TAG: u64 = 1 << 63;

/// A source group's functions (see `Group`).
pub type SrcTakeFn = unsafe extern "C" fn(data: *mut std::ffi::c_void, token: u64) -> bool;
pub type SrcFlushFn = unsafe extern "C" fn(data: *mut std::ffi::c_void);

/// An embedder's sources: `take(data, token)` is called during the poll for each event of a
/// source armed with `token` (true: the waiter took it; false: it no longer waits, the readiness
/// is kept for the next wait); `flush(data)` once after the poll when some `take` returned true.
/// Neither may call into the loop or PHP from `take`.
#[derive(Clone, Copy)]
pub struct Group {
    pub take: SrcTakeFn,
    pub flush: SrcFlushFn,
    pub data: *mut std::ffi::c_void,
    /// counted in the driver's live callbacks (the loop runs while it is)
    pub referenced: bool,
    /// a `take` returned true since the last flush
    pending: bool,
}

/// A source to arm or disarm: an fd (a SOCKET on Windows) and what the wait is for (READ, WRITE).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SrcFd {
    pub fd: i64,
    pub events: u32,
}

#[derive(Clone, Copy, Default)]
struct SrcEntry {
    /// the group (1-based) of the waiter / of the last arm
    group: u32,
    /// registered with the poller (Unix)
    registered: bool,
    /// readiness seen while nobody waited for it (READ, WRITE, ERR)
    ready: u8,
    /// what the armed waiter waits for (0: not armed)
    want: u8,
    token: u64,
}

/// Bumped in fork() children (pthread_atfork).
pub static FORK_GEN: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
extern "C" fn atfork_child() {
    FORK_GEN.fetch_add(1, Ordering::SeqCst);
}

/// MINIT: the fork hook (once per process).
pub fn minit() {
    #[cfg(unix)]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| unsafe {
            libc::pthread_atfork(None, None, Some(atfork_child));
        });
    }
}

#[derive(Default)]
pub struct FdEntry {
    pub readers: Vec<u64>,
    pub writers: Vec<u64>,
    /// interest registered with the poller
    registered: u8,
    /// the poller refused the fd: reported ready on every poll
    pub always: bool,
    /// Windows: the fd is a SOCKET
    pub socket: bool,
    dirty: bool,
    /// the PHP resource the fd belongs to (0: a raw handle): another resource on the same fd
    /// number means the old one was closed (its registration went with it)
    res: i64,
}

impl FdEntry {
    fn want(&self) -> u8 {
        (if self.readers.is_empty() { 0 } else { READ }) | (if self.writers.is_empty() { 0 } else { WRITE })
    }
}

pub struct Reactor {
    pub fds: HashMap<i64, FdEntry, BuildHasherDefault<super::IdHasher>>,
    /// the active readable callbacks of PHP streams (whose read buffers a blocking poll checks)
    pub stream_readers: usize,
    /// the entries marked `always` (so that a poll looks for them only when there are some)
    n_always: usize,
    dirty: Vec<i64>,
    #[cfg(unix)]
    poller: i32,
    #[cfg(unix)]
    fork_gen: u64,
    /// the read end of the signal wake pipe, polled too
    wake: i64,
    #[cfg(target_os = "linux")]
    events: Vec<libc::epoll_event>,
    #[cfg(all(unix, not(target_os = "linux")))]
    events: Vec<Kev>,
    /// the deadline (loop clock) the wait timer is armed for, while it may be (it has not been
    /// seen firing); NaN: armed for a wait without a known deadline (macOS)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    timer_armed: Option<f64>,
    /// Linux: the wait timer's timerfd, in the poller (-1: not made yet; -2: unavailable)
    #[cfg(target_os = "linux")]
    timer_fd: i32,
    /// the sources, by fd
    src: HashMap<i64, SrcEntry, BuildHasherDefault<super::IdHasher>>,
    /// the source groups (id = index + 1; None: closed)
    groups: Vec<Option<Group>>,
    /// a group is waiting for its flush
    flush_pending: bool,
    /// the sources armed for a wait
    armed: usize,
}

/// macOS: the ident of the wait timer (EVFILT_TIMER idents are not fds)
#[cfg(target_os = "macos")]
const WAIT_TIMER: usize = usize::MAX;

/// macOS: kevent64(), whose KEVENT_FLAG_IMMEDIATE makes a poll without waiting cost ~0.15 µs
/// (kevent() with a zero timeout costs ~12 µs: it sets up a kernel timer). A kqueue takes one
/// flavour of kevent call only (another one fails with EINVAL): every call goes through `kev`.
#[cfg(target_os = "macos")]
type Kev = libc::kevent64_s;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
type Kev = libc::kevent;

/// A change for `kev` (plain data otherwise zero).
#[cfg(all(unix, not(target_os = "linux")))]
fn kev_change(ident: i64, filter: i16, flags: u16) -> Kev {
    // SAFETY: kevent structures are plain data
    let mut k: Kev = unsafe { std::mem::zeroed() };
    k.ident = ident as _;
    k.filter = filter;
    k.flags = flags;
    k
}

/// kevent64()/kevent(): `changes`, then up to `events.len()` events; waits for `timeout` (None:
/// no limit; Some(zero): not at all).
#[cfg(all(unix, not(target_os = "linux")))]
fn kev(kq: i32, changes: &[Kev], events: &mut [Kev], timeout: Option<&libc::timespec>) -> i32 {
    #[cfg(target_os = "macos")]
    {
        let (flags, tsp) = match timeout {
            Some(t) if t.tv_sec == 0 && t.tv_nsec == 0 => (libc::KEVENT_FLAG_IMMEDIATE, std::ptr::null()),
            Some(t) => (0, t as *const libc::timespec),
            None => (0, std::ptr::null()),
        };
        // SAFETY: valid buffers of the given lengths
        unsafe { libc::kevent64(kq, changes.as_ptr(), changes.len() as i32, events.as_mut_ptr(), events.len() as i32, flags, tsp) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let tsp = timeout.map_or(std::ptr::null(), |t| t as *const libc::timespec);
        // SAFETY: valid buffers of the given lengths
        unsafe { libc::kevent(kq, changes.as_ptr(), changes.len() as i32, events.as_mut_ptr(), events.len() as i32, tsp) }
    }
}

/// A ready fd: (fd, readable, writable)
pub type Ready = (i64, bool, bool);

impl Reactor {
    pub fn new() -> Reactor {
        Reactor {
            fds: HashMap::default(),
            stream_readers: 0,
            n_always: 0,
            dirty: Vec::new(),
            #[cfg(unix)]
            poller: -1,
            #[cfg(unix)]
            fork_gen: FORK_GEN.load(Ordering::Relaxed),
            wake: -1,
            #[cfg(unix)]
            events: Vec::new(),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            timer_armed: None,
            #[cfg(target_os = "linux")]
            timer_fd: -1,
            src: HashMap::default(),
            groups: Vec::new(),
            flush_pending: false,
            armed: 0,
        }
    }

    /// Whether any fd is watched (or a wake pipe is set).
    pub fn has_fds(&self) -> bool {
        !self.fds.is_empty() || self.wake >= 0
    }

    pub fn add(&mut self, fd: i64, socket: bool, res: i64, id: u64, dir: u8) {
        let e = self.fds.entry(fd).or_default();
        e.socket = socket;
        if e.want() == 0 || e.res != res {
            // the fd may have been closed and reused since it was registered: register anew
            e.registered = 0;
            if e.always {
                e.always = false;
                self.n_always -= 1;
            }
            e.res = res;
        }
        if dir == READ {
            e.readers.push(id);
        } else {
            e.writers.push(id);
        }
        if !e.dirty {
            e.dirty = true;
            self.dirty.push(fd);
        }
    }

    pub fn del(&mut self, fd: i64, id: u64, dir: u8) {
        let Some(e) = self.fds.get_mut(&fd) else { return };
        let list = if dir == READ { &mut e.readers } else { &mut e.writers };
        if let Some(p) = list.iter().position(|&x| x == id) {
            list.remove(p);
        }
        if !e.dirty {
            e.dirty = true;
            self.dirty.push(fd);
        }
    }

    /// Sets (or clears with -1) the signal wake pipe's read end.
    pub fn set_wake(&mut self, fd: i64) {
        if self.wake == fd {
            return;
        }
        #[cfg(unix)]
        if self.poller >= 0 {
            if self.wake >= 0 {
                self.ctl(self.wake, READ, 0);
            }
            if fd >= 0 {
                self.ctl(fd, 0, READ);
            }
        }
        self.wake = fd;
    }

    /// A new source group; its id (never 0).
    pub fn src_group(&mut self, take: SrcTakeFn, flush: SrcFlushFn, data: *mut std::ffi::c_void) -> u32 {
        let g = Group { take, flush, data, referenced: false, pending: false };
        match self.groups.iter().position(Option::is_none) {
            Some(i) => {
                self.groups[i] = Some(g);
                i as u32 + 1
            }
            None => {
                self.groups.push(Some(g));
                self.groups.len() as u32
            }
        }
    }

    fn group_mut(&mut self, g: u32) -> Option<&mut Group> {
        self.groups.get_mut((g as usize).wrapping_sub(1)).and_then(Option::as_mut)
    }

    /// Closes a group: its sources are forgotten (their kernel registrations stay until the fds
    /// close: events for unknown sources are ignored, and a later arm registers anew). Returns
    /// whether it was referenced.
    pub fn src_close(&mut self, g: u32) -> bool {
        let Some(slot) = self.groups.get_mut((g as usize).wrapping_sub(1)) else { return false };
        let Some(old) = slot.take() else { return false };
        let mut gone = 0;
        self.src.retain(|_, e| {
            let keep = e.group != g;
            if !keep && e.want != 0 {
                gone += 1;
            }
            keep
        });
        self.armed -= gone;
        old.referenced
    }

    /// Sets whether a group is referenced; Some(true) when that changed it.
    pub fn src_set_referenced(&mut self, g: u32, on: bool) -> Option<bool> {
        let gr = self.group_mut(g)?;
        let changed = gr.referenced != on;
        gr.referenced = on;
        Some(changed)
    }

    /// Arms the sources `fds` of group `g` for the wait `token`. Ok(true): one of them already
    /// had readiness for what it waits for (consumed; nothing is armed): the waiter retries its
    /// I/O instead of waiting. Err: the poller refused a source (errno).
    pub fn src_arm(&mut self, g: u32, fds: &[SrcFd], token: u64) -> Result<bool, i32> {
        if self.group_mut(g).is_none() {
            return Err(libc::EBADF);
        }
        #[cfg(unix)]
        self.check_fork();
        let mut hit = false;
        for p in fds.iter().filter(|p| p.fd >= 0) {
            if let Some(e) = self.src.get_mut(&p.fd) {
                let bits = e.ready & (p.events as u8 | ERR);
                e.ready &= !bits;
                hit |= bits != 0;
            }
        }
        if hit {
            return Ok(true);
        }
        for (k, p) in fds.iter().enumerate().filter(|(_, p)| p.fd >= 0) {
            if let Err(err) = self.src_watch(g, p, token) {
                for q in fds[..k].iter().filter(|q| q.fd >= 0) {
                    if let Some(e) = self.src.get_mut(&q.fd).filter(|e| e.token == token && e.want != 0) {
                        e.want = 0;
                        self.armed -= 1;
                    }
                }
                return Err(err);
            }
        }
        Ok(false)
    }

    /// The wait `token` over `fds` ended: its sources are no longer armed for it.
    pub fn src_disarm(&mut self, fds: &[SrcFd], token: u64) {
        for p in fds {
            if let Some(e) = self.src.get_mut(&p.fd).filter(|e| e.token == token && e.want != 0) {
                e.want = 0;
                self.armed -= 1;
            }
        }
    }

    /// The source `fd` is about to be closed: its state goes (a later fd with its number is
    /// registered anew).
    pub fn src_forget(&mut self, fd: i64) {
        if self.src.remove(&fd).is_some_and(|e| e.want != 0) {
            self.armed -= 1;
        }
    }

    /// The number of sources armed for a wait.
    #[inline]
    pub fn src_armed(&self) -> usize {
        self.armed
    }

    /// Whether a poll left a group to flush.
    #[inline]
    pub fn flush_pending(&self) -> bool {
        self.flush_pending
    }

    /// The next group a poll left to flush (its flag cleared): (flush, data).
    pub fn next_flush(&mut self) -> Option<(SrcFlushFn, *mut std::ffi::c_void)> {
        if !self.flush_pending {
            return None;
        }
        for g in self.groups.iter_mut().flatten() {
            if g.pending {
                g.pending = false;
                return Some((g.flush, g.data));
            }
        }
        self.flush_pending = false;
        None
    }

    /// An event `bits` of the source `fd`: offered to its armed waiter, else kept as ready bits.
    fn src_event(&mut self, fd: i64, bits: u8) {
        let Some(e) = self.src.get_mut(&fd) else { return };
        if e.want != 0 && bits & (e.want | ERR) != 0 {
            if let Some(g) = self.groups.get_mut((e.group as usize).wrapping_sub(1)).and_then(Option::as_mut) {
                // SAFETY: the group's function, which must not call into the loop
                if unsafe { (g.take)(g.data, e.token) } {
                    g.pending = true;
                    self.flush_pending = true;
                    e.ready |= bits & !(e.want | ERR);
                    e.want = 0;
                    self.armed -= 1;
                    return;
                }
            }
        }
        e.ready |= bits;
        #[cfg(windows)]
        {
            // level-triggered: an armed source nobody takes would be reported at every poll
            if e.want != 0 {
                e.want = 0;
                self.armed -= 1;
            }
        }
    }

    /// Registers the source `p` with the poller if needed and arms it.
    #[cfg(target_os = "linux")]
    fn src_watch(&mut self, g: u32, p: &SrcFd, token: u64) -> Result<(), i32> {
        let registered = self.src.get(&p.fd).is_some_and(|e| e.registered);
        if !registered {
            if !self.ensure_poller() {
                return Err(errno());
            }
            let mut ev = libc::epoll_event { events: (libc::EPOLLIN | libc::EPOLLOUT | libc::EPOLLRDHUP | libc::EPOLLET) as u32, u64: p.fd as u64 | SRC_TAG };
            // SAFETY: a valid event for the fd
            if unsafe { libc::epoll_ctl(self.poller, libc::EPOLL_CTL_ADD, p.fd as i32, &mut ev) } != 0 {
                let err = errno();
                // registered by a closed group of this poller: still the same registration
                if err != libc::EEXIST {
                    return Err(err);
                }
            }
        }
        let e = self.src.entry(p.fd).or_default();
        let want = p.events as u8 & (READ | WRITE);
        self.armed = self.armed + (want != 0) as usize - (e.want != 0) as usize;
        *e = SrcEntry { group: g, registered: true, want, token, ..*e };
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn src_watch(&mut self, g: u32, p: &SrcFd, token: u64) -> Result<(), i32> {
        let registered = self.src.get(&p.fd).is_some_and(|e| e.registered);
        let write = p.events as u8 & WRITE != 0;
        if !registered || write {
            if !self.ensure_poller() {
                return Err(errno());
            }
            let ch = [kev_change(p.fd, libc::EVFILT_READ, libc::EV_ADD | libc::EV_CLEAR), kev_change(p.fd, libc::EVFILT_WRITE, libc::EV_ADD | libc::EV_ONESHOT)];
            let changes: &[Kev] = match (registered, write) {
                (false, true) => &ch,
                (false, false) => &ch[..1],
                (true, _) => &ch[1..],
            };
            if kev(self.poller, changes, &mut [], None) < 0 {
                return Err(errno());
            }
        }
        let e = self.src.entry(p.fd).or_default();
        let want = p.events as u8 & (READ | WRITE);
        self.armed = self.armed + (want != 0) as usize - (e.want != 0) as usize;
        *e = SrcEntry { group: g, registered: true, want, token, ..*e };
        Ok(())
    }

    #[cfg(windows)]
    fn src_watch(&mut self, g: u32, p: &SrcFd, token: u64) -> Result<(), i32> {
        let e = self.src.entry(p.fd).or_default();
        let want = p.events as u8 & (READ | WRITE);
        self.armed = self.armed + (want != 0) as usize - (e.want != 0) as usize;
        *e = SrcEntry { group: g, registered: true, want, token, ..*e };
        Ok(())
    }

    /// Whether some watched fd is always ready (the poll must not block).
    pub fn has_always(&self) -> bool {
        self.n_always > 0 && self.fds.values().any(|e| e.always && e.want() != 0)
    }

    /// Polls for readiness for at most `timeout` seconds (None: no limit), which end at
    /// `deadline` on the loop clock when it is known; appends the ready fds to `out`; returns
    /// whether the wake pipe was signalled. EINTR returns at once, empty.
    pub fn poll(&mut self, timeout: Option<f64>, deadline: Option<f64>, out: &mut Vec<Ready>) -> bool {
        self.check_fork();
        self.apply();
        let mut timeout = timeout;
        if self.n_always > 0 {
            for (&fd, e) in self.fds.iter() {
                if e.always {
                    let w = e.want();
                    if w != 0 {
                        out.push((fd, w & READ != 0, w & WRITE != 0));
                        timeout = Some(0.0);
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            self.poll_windows(timeout, out)
        }
        #[cfg(unix)]
        {
            let mut woke = false;
            #[cfg(target_os = "macos")]
            if timeout.is_some_and(|t| t > 0.0) {
                self.ensure_poller();
            }
            if self.poller < 0 {
                // nothing registered with a poller: sleep
                sleep(timeout);
                return false;
            }
            if timeout == Some(0.0) && self.fds.is_empty() && self.src.is_empty() && self.wake < 0 {
                // nothing could be reported: no system call (an expired wait timer's event, if
                // any, stays queued for the next wait)
                return false;
            }
            self.poll_os(timeout, deadline, out, &mut woke);
            woke
        }
    }

    #[cfg(unix)]
    fn check_fork(&mut self) {
        let g = FORK_GEN.load(Ordering::Relaxed);
        if g == self.fork_gen {
            return;
        }
        self.fork_gen = g;
        if self.poller >= 0 {
            // epoll: the instance is shared with the parent, never touch it again; kqueue: the
            // fd is not inherited at all (its number may be reused): never close it
            #[cfg(target_os = "linux")]
            unsafe {
                libc::close(self.poller)
            };
            self.poller = -1;
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                self.timer_armed = None;
            }
        }
        // the timerfd is shared with the parent too: arming it would move the parent's timer
        #[cfg(target_os = "linux")]
        if self.timer_fd >= 0 {
            unsafe { libc::close(self.timer_fd) };
            self.timer_fd = -1;
        }
        for (&fd, e) in self.fds.iter_mut() {
            e.registered = 0;
            e.always = false;
            if !e.dirty {
                e.dirty = true;
                self.dirty.push(fd);
            }
        }
        self.n_always = 0;
        // the sources are registered anew at their next wait (an armed one now: at once)
        let armed: Vec<SrcFd> = self.src.iter().filter(|(_, e)| e.want != 0).map(|(&fd, e)| SrcFd { fd, events: e.want as u32 }).collect();
        for e in self.src.values_mut() {
            e.registered = false;
        }
        for p in armed {
            let (g, token) = self.src.get(&p.fd).map(|e| (e.group, e.token)).unwrap_or_default();
            if self.src_watch(g, &p, token).is_err() {
                // report it to its waiter, which finds out what is wrong when it retries
                self.src_event(p.fd, READ | WRITE | ERR);
            }
        }
    }

    #[cfg(windows)]
    fn check_fork(&mut self) {}

    /// Brings the poller's interest in line with the watchers.
    fn apply(&mut self) {
        let dirty = std::mem::take(&mut self.dirty);
        for fd in dirty {
            let Some(e) = self.fds.get_mut(&fd) else { continue };
            e.dirty = false;
            let want = e.want();
            if want == 0 {
                let had = e.registered;
                if e.always {
                    self.n_always -= 1;
                }
                self.fds.remove(&fd);
                #[cfg(unix)]
                if had != 0 {
                    self.ctl(fd, had, 0);
                }
                #[cfg(windows)]
                let _ = had;
                continue;
            }
            #[cfg(unix)]
            {
                let had = e.registered;
                if e.always || had == want {
                    continue;
                }
                let ok = self.ctl(fd, had, want);
                let e = self.fds.get_mut(&fd).unwrap();
                if ok {
                    e.registered = want;
                } else {
                    // refused (a regular file, a closed fd): select would report it ready
                    e.always = true;
                    e.registered = 0;
                    self.n_always += 1;
                }
            }
            #[cfg(windows)]
            {
                e.registered = want;
            }
        }
    }

    #[cfg(unix)]
    fn ensure_poller(&mut self) -> bool {
        if self.poller >= 0 {
            return true;
        }
        #[cfg(target_os = "linux")]
        let p = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        #[cfg(not(target_os = "linux"))]
        let p = unsafe {
            let k = libc::kqueue();
            if k >= 0 {
                libc::fcntl(k, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            k
        };
        if p < 0 {
            return false;
        }
        self.poller = p;
        if self.wake >= 0 {
            let w = self.wake;
            self.ctl(w, 0, READ);
        }
        true
    }

    /// Changes the interest in `fd` from `had` to `want`; false when the poller refuses it.
    #[cfg(target_os = "linux")]
    fn ctl(&mut self, fd: i64, had: u8, want: u8) -> bool {
        if !self.ensure_poller() {
            return false;
        }
        let mut ev = libc::epoll_event { events: 0, u64: fd as u64 };
        if want & READ != 0 {
            ev.events |= libc::EPOLLIN as u32;
        }
        if want & WRITE != 0 {
            ev.events |= libc::EPOLLOUT as u32;
        }
        let fd = fd as i32;
        unsafe {
            if want == 0 {
                libc::epoll_ctl(self.poller, libc::EPOLL_CTL_DEL, fd, &mut ev);
                return true;
            }
            let op = if had == 0 { libc::EPOLL_CTL_ADD } else { libc::EPOLL_CTL_MOD };
            if libc::epoll_ctl(self.poller, op, fd, &mut ev) == 0 {
                return true;
            }
            // reconcile with the kernel's view (an fd closed and reused, a dup...)
            let err = *libc::__errno_location();
            let retry = match err {
                libc::EEXIST => libc::EPOLL_CTL_MOD,
                libc::ENOENT => libc::EPOLL_CTL_ADD,
                _ => return false,
            };
            libc::epoll_ctl(self.poller, retry, fd, &mut ev) == 0
        }
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn ctl(&mut self, fd: i64, had: u8, want: u8) -> bool {
        if !self.ensure_poller() {
            return false;
        }
        let mut changes = [kev_change(fd, 0, 0); 2];
        let mut n = 0;
        for (bit, filter) in [(READ, libc::EVFILT_READ), (WRITE, libc::EVFILT_WRITE)] {
            // a fresh registration (had 0) also drops a filter left from a reused fd number
            if (had ^ want) & bit != 0 || (had == 0 && want & bit == 0) {
                changes[n] = kev_change(fd, filter, (if want & bit != 0 { libc::EV_ADD } else { libc::EV_DELETE }) | libc::EV_RECEIPT);
                n += 1;
            }
        }
        if n == 0 {
            return true;
        }
        let mut res = [kev_change(0, 0, 0); 2];
        let r = kev(self.poller, &changes[..n], &mut res[..n], None);
        if r < 0 {
            return want == 0;
        }
        for ev in &res[..r as usize] {
            if ev.flags & libc::EV_ERROR != 0 && ev.data != 0 {
                // a delete of a filter the kernel dropped (closed fd) is fine
                if ev.flags & libc::EV_DELETE != 0 || want == 0 {
                    continue;
                }
                let added = ev.filter == libc::EVFILT_READ && want & READ != 0 || ev.filter == libc::EVFILT_WRITE && want & WRITE != 0;
                if added {
                    return false;
                }
            }
        }
        true
    }

    /// Linux: arms the wait timer for `at` (loop clock) unless it is armed for it already; false
    /// when there is no timerfd.
    #[cfg(target_os = "linux")]
    fn arm_timer(&mut self, at: f64) -> bool {
        if self.timer_fd == -1 {
            let fd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK) };
            let mut ev = libc::epoll_event { events: libc::EPOLLIN as u32, u64: fd as u64 };
            if fd >= 0 && unsafe { libc::epoll_ctl(self.poller, libc::EPOLL_CTL_ADD, fd, &mut ev) } == 0 {
                self.timer_fd = fd;
            } else {
                if fd >= 0 {
                    unsafe { libc::close(fd) };
                }
                self.timer_fd = -2;
            }
        }
        if self.timer_fd < 0 {
            return false;
        }
        if self.timer_armed == Some(at) {
            return true;
        }
        // rounded up: an early wake-up would spin until the timer is due
        let ns = (at.fract() * 1e9).ceil();
        let (sec, ns) = if ns >= 1e9 { (at.trunc() + 1.0, 0.0) } else { (at.trunc(), ns) };
        let spec = libc::itimerspec {
            it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
            // at least 1 ns: zero disarms
            it_value: libc::timespec { tv_sec: sec.min(i64::MAX as f64) as libc::time_t, tv_nsec: if sec <= 0.0 { ns.max(1.0) } else { ns } as libc::c_long },
        };
        if unsafe { libc::timerfd_settime(self.timer_fd, libc::TFD_TIMER_ABSTIME, &spec, std::ptr::null_mut()) } != 0 {
            return false;
        }
        self.timer_armed = Some(at);
        true
    }

    #[cfg(target_os = "linux")]
    fn poll_os(&mut self, timeout: Option<f64>, deadline: Option<f64>, out: &mut Vec<Ready>, woke: &mut bool) {
        let n = self.fds.len() + self.src.len() + 2;
        if self.events.len() < n.min(1024) {
            self.events.resize(n.min(1024), libc::epoll_event { events: 0, u64: 0 });
        }
        let ms = match timeout {
            None => {
                if self.timer_armed.take().is_some() {
                    // a stale timer would end a wait without timeout early
                    let off: libc::itimerspec = unsafe { std::mem::zeroed() };
                    unsafe { libc::timerfd_settime(self.timer_fd, 0, &off, std::ptr::null_mut()) };
                }
                -1
            }
            // a non-blocking poll leaves the timer armed (it ends no wait)
            Some(t) if t <= 0.0 => 0,
            Some(_) if deadline.is_some_and(|at| self.arm_timer(at)) => -1,
            // whole milliseconds, rounded up (an early wake-up would spin until the timer is due)
            Some(t) => (t * 1000.0).ceil().min(i32::MAX as f64) as i32,
        };
        let r = unsafe { libc::epoll_wait(self.poller, self.events.as_mut_ptr(), self.events.len() as i32, ms) };
        if r <= 0 {
            return;
        }
        for i in 0..r as usize {
            let ev = self.events[i];
            if ev.u64 & SRC_TAG != 0 {
                let mut bits = 0;
                if ev.events & libc::EPOLLIN as u32 != 0 {
                    bits |= READ;
                }
                if ev.events & libc::EPOLLOUT as u32 != 0 {
                    bits |= WRITE;
                }
                if ev.events & (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32 != 0 {
                    bits |= ERR | READ;
                }
                self.src_event((ev.u64 & !SRC_TAG) as i64, bits);
                continue;
            }
            let fd = ev.u64 as i64;
            if fd == self.wake {
                *woke = true;
                continue;
            }
            if fd == self.timer_fd as i64 {
                // expired: its count is read (else it stays readable)
                let mut n = 0u64;
                unsafe { libc::read(self.timer_fd, &mut n as *mut u64 as *mut _, 8) };
                self.timer_armed = None;
                continue;
            }
            let err = ev.events & (libc::EPOLLERR | libc::EPOLLHUP) as u32 != 0;
            let rd = err || ev.events & libc::EPOLLIN as u32 != 0;
            let wr = err || ev.events & libc::EPOLLOUT as u32 != 0;
            out.push((fd, rd, wr));
        }
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    fn poll_os(&mut self, timeout: Option<f64>, deadline: Option<f64>, out: &mut Vec<Ready>, woke: &mut bool) {
        let n = (2 * (self.fds.len() + self.src.len()) + 2).min(1024);
        if self.events.len() < n {
            self.events.resize(n, kev_change(0, 0, 0));
        }
        let ts = timeout.map(|t| {
            let t = t.max(0.0);
            libc::timespec { tv_sec: t.trunc() as libc::time_t, tv_nsec: (t.fract() * 1e9) as libc::c_long }
        });
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut wait = ts.as_ref();
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut change: &[Kev] = &[];
        #[cfg(target_os = "macos")]
        let timer;
        #[cfg(target_os = "macos")]
        match timeout {
            Some(t) if t > 0.0 => {
                // the wait itself has no timeout: the timer ends it; (re)armed unless it is
                // armed for this deadline already (NaN never compares equal)
                wait = None;
                let at = deadline.unwrap_or(f64::NAN);
                if self.timer_armed != Some(at) {
                    let mut k = kev_change(WAIT_TIMER as i64, libc::EVFILT_TIMER, libc::EV_ADD | libc::EV_ONESHOT);
                    k.fflags = libc::NOTE_NSECONDS | libc::NOTE_CRITICAL;
                    k.data = (t * 1e9).ceil().min(i64::MAX as f64) as _;
                    timer = [k];
                    change = &timer;
                    self.timer_armed = Some(at);
                }
            }
            None if self.timer_armed.is_some() => {
                // a stale timer would end a wait without timeout early (ENOENT if it
                // fired: ignored); a non-blocking poll leaves it armed (it ends no wait)
                timer = [kev_change(WAIT_TIMER as i64, libc::EVFILT_TIMER, libc::EV_DELETE)];
                change = &timer;
                self.timer_armed = None;
            }
            _ => {}
        }
        let r = kev(self.poller, change, &mut self.events, wait);
        if r <= 0 {
            return;
        }
        let start = out.len();
        for i in 0..r as usize {
            let ev = self.events[i];
            let fd = ev.ident as i64;
            #[cfg(target_os = "macos")]
            if ev.filter == libc::EVFILT_TIMER {
                self.timer_armed = None;
                continue;
            }
            if fd == self.wake {
                *woke = true;
                continue;
            }
            if self.src.contains_key(&fd) && (ev.filter == libc::EVFILT_READ || ev.filter == libc::EVFILT_WRITE) {
                let mut bits = if ev.filter == libc::EVFILT_READ { READ } else { WRITE };
                if ev.flags & (libc::EV_EOF | libc::EV_ERROR) != 0 {
                    bits |= ERR;
                }
                self.src_event(fd, bits);
                continue;
            }
            if ev.flags & libc::EV_ERROR != 0 {
                continue;
            }
            let rd = ev.filter == libc::EVFILT_READ;
            let wr = ev.filter == libc::EVFILT_WRITE;
            // one entry per fd
            if let Some(o) = out[start..].iter_mut().find(|o| o.0 == fd) {
                o.1 |= rd;
                o.2 |= wr;
            } else {
                out.push((fd, rd, wr));
            }
        }
    }

    #[cfg(windows)]
    fn poll_windows(&mut self, timeout: Option<f64>, out: &mut Vec<Ready>) -> bool {
        use windows_sys::Win32::Networking::WinSock::{WSAPoll, POLLERR, POLLHUP, POLLRDNORM, POLLWRNORM, WSAPOLLFD};
        let mut pfds: Vec<WSAPOLLFD> = Vec::new();
        let mut others = false;
        for (&fd, e) in self.fds.iter() {
            if e.always {
                continue;
            }
            let w = e.want();
            if !e.socket {
                others = true;
                let (rd, wr) = win::handle_ready(fd, w);
                if rd || wr {
                    out.push((fd, rd, wr));
                }
                continue;
            }
            let mut ev = 0i16;
            if w & READ != 0 {
                ev |= POLLRDNORM as i16;
            }
            if w & WRITE != 0 {
                ev |= POLLWRNORM as i16;
            }
            pfds.push(WSAPOLLFD { fd: fd as usize, events: ev, revents: 0 });
        }
        // the armed sources (sockets), after the watchers' fds
        let n_fds = pfds.len();
        for (&fd, e) in self.src.iter() {
            if e.want == 0 {
                continue;
            }
            let mut ev = 0i16;
            if e.want & READ != 0 {
                ev |= POLLRDNORM as i16;
            }
            if e.want & WRITE != 0 {
                ev |= POLLWRNORM as i16;
            }
            pfds.push(WSAPOLLFD { fd: fd as usize, events: ev, revents: 0 });
        }
        let mut timeout = timeout;
        if !out.is_empty() {
            timeout = Some(0.0);
        } else if others {
            // pipes and consoles cannot be waited for with sockets: poll them every 10 ms
            timeout = Some(timeout.map_or(0.01, |t| t.min(0.01)));
        }
        let ms = match timeout {
            None => -1,
            Some(t) if t <= 0.0 => 0,
            Some(t) => (t * 1000.0).ceil().min(i32::MAX as f64) as i32,
        };
        if pfds.is_empty() {
            if ms != 0 {
                sleep(if ms < 0 { None } else { Some(ms as f64 / 1000.0) });
            }
            return false;
        }
        let r = unsafe { WSAPoll(pfds.as_mut_ptr(), pfds.len() as u32, ms) };
        if r <= 0 {
            return false;
        }
        for (i, p) in pfds.iter().enumerate() {
            if p.revents == 0 {
                continue;
            }
            let err = p.revents & (POLLERR | POLLHUP) as i16 != 0;
            let (rd, wr) = (err || p.revents & POLLRDNORM as i16 != 0, err || p.revents & POLLWRNORM as i16 != 0);
            if i >= n_fds {
                let bits = (if rd { READ } else { 0 }) | (if wr { WRITE } else { 0 }) | (if err { ERR } else { 0 });
                self.src_event(p.fd as i64, bits);
                continue;
            }
            out.push((p.fd as i64, rd, wr));
        }
        false
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.poller >= 0 && self.fork_gen == FORK_GEN.load(Ordering::Relaxed) {
            unsafe { libc::close(self.poller) };
        }
        #[cfg(target_os = "linux")]
        if self.timer_fd >= 0 {
            unsafe { libc::close(self.timer_fd) };
        }
    }
}

/// The last OS error number.
#[cfg(unix)]
fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO)
}

/// Sleeps (no fd to wait for): usleep() as Revolt's stream_select driver does; returns early on
/// signals (EINTR).
fn sleep(timeout: Option<f64>) {
    match timeout {
        Some(t) if t <= 0.0 => {}
        #[cfg(unix)]
        t => unsafe {
            let t = t.unwrap_or(1e9).min(1e9);
            let ts = libc::timespec { tv_sec: t.trunc() as libc::time_t, tv_nsec: (t.fract() * 1e9) as libc::c_long };
            libc::nanosleep(&ts, std::ptr::null_mut());
        },
        #[cfg(windows)]
        t => std::thread::sleep(std::time::Duration::from_secs_f64(t.unwrap_or(1e9).min(1e9))),
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    extern "C" {
        fn _get_osfhandle(fd: i32) -> isize;
    }

    /// Readiness of a non-socket CRT fd: pipes have data (or are broken), files always,
    /// consoles have input events.
    pub fn handle_ready(fd: i64, want: u8) -> (bool, bool) {
        let h = unsafe { _get_osfhandle(fd as i32) } as HANDLE;
        if h == INVALID_HANDLE_VALUE {
            return (want & super::READ != 0, want & super::WRITE != 0);
        }
        let wr = want & super::WRITE != 0;
        let rd = want & super::READ != 0
            && match unsafe { GetFileType(h) } {
                FILE_TYPE_PIPE => {
                    let mut avail = 0u32;
                    let ok = unsafe { PeekNamedPipe(h, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) };
                    ok == 0 || avail > 0
                }
                FILE_TYPE_DISK => true,
                FILE_TYPE_CHAR => (unsafe { WaitForSingleObject(h, 0) }) == WAIT_OBJECT_0,
                _ => true,
            };
        (rd, wr)
    }
}
