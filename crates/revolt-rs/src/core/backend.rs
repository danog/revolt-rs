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
//! - macOS: timed waits go through a one-shot EVFILT_TIMER with NOTE_CRITICAL (kqueue even when
//!   no fd is watched): the system's timer coalescing otherwise delays nanosleep()/kevent()
//!   wake-ups by up to 10% of the timeout (more for background processes).
//! - Linux: timed waits go through a timerfd armed for the deadline (absolute, CLOCK_MONOTONIC:
//!   the loop clock), polled with the fds: epoll_wait()'s own timeout costs a kernel timer per
//!   call and has whole milliseconds only.
//! - Both wait timers stay armed for their deadline across polls: a wait for the same deadline
//!   (the same next timer) changes nothing, so it costs no kernel timer work.

use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::sync::atomic::AtomicU64;
#[cfg(unix)]
use std::sync::atomic::Ordering;

pub const READ: u8 = 1;
pub const WRITE: u8 = 2;

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
    events: Vec<libc::kevent>,
    /// the deadline (loop clock) the wait timer is armed for, while it may be (it has not been
    /// seen firing); NaN: armed for a wait without a known deadline (macOS)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    timer_armed: Option<f64>,
    /// Linux: the wait timer's timerfd, in the poller (-1: not made yet; -2: unavailable)
    #[cfg(target_os = "linux")]
    timer_fd: i32,
}

/// macOS: the ident of the wait timer (EVFILT_TIMER idents are not fds)
#[cfg(target_os = "macos")]
const WAIT_TIMER: usize = usize::MAX;

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
        let mut changes: [libc::kevent; 2] = unsafe { std::mem::zeroed() };
        let mut n = 0;
        for (bit, filter) in [(READ, libc::EVFILT_READ), (WRITE, libc::EVFILT_WRITE)] {
            // a fresh registration (had 0) also drops a filter left from a reused fd number
            if (had ^ want) & bit != 0 || (had == 0 && want & bit == 0) {
                let c = &mut changes[n];
                c.ident = fd as usize;
                c.filter = filter;
                c.flags = (if want & bit != 0 { libc::EV_ADD } else { libc::EV_DELETE }) | libc::EV_RECEIPT;
                n += 1;
            }
        }
        if n == 0 {
            return true;
        }
        let mut res: [libc::kevent; 2] = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::kevent(self.poller, changes.as_ptr(), n as i32, res.as_mut_ptr(), n as i32, std::ptr::null()) };
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
        let n = self.fds.len() + 2;
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
        for ev in &self.events[..r as usize] {
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
        let n = (2 * self.fds.len() + 2).min(1024);
        if self.events.len() < n {
            self.events.resize(n, unsafe { std::mem::zeroed() });
        }
        let ts;
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut tsp = match timeout {
            None => std::ptr::null(),
            Some(t) => {
                let t = t.max(0.0);
                ts = libc::timespec { tv_sec: t.trunc() as libc::time_t, tv_nsec: (t.fract() * 1e9) as libc::c_long };
                &ts as *const libc::timespec
            }
        };
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut change: libc::kevent = unsafe { std::mem::zeroed() };
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut nchanges = 0;
        #[cfg(target_os = "macos")]
        {
            change.ident = WAIT_TIMER;
            change.filter = libc::EVFILT_TIMER;
            match timeout {
                Some(t) if t > 0.0 => {
                    // the wait itself has no timeout: the timer ends it; (re)armed unless it is
                    // armed for this deadline already (NaN never compares equal)
                    tsp = std::ptr::null();
                    let at = deadline.unwrap_or(f64::NAN);
                    if self.timer_armed != Some(at) {
                        change.flags = libc::EV_ADD | libc::EV_ONESHOT;
                        change.fflags = libc::NOTE_NSECONDS | libc::NOTE_CRITICAL;
                        change.data = (t * 1e9).ceil().min(i64::MAX as f64) as isize;
                        nchanges = 1;
                        self.timer_armed = Some(at);
                    }
                }
                None if self.timer_armed.is_some() => {
                    // a stale timer would end a wait without timeout early (ENOENT if it
                    // fired: ignored); a non-blocking poll leaves it armed (it ends no wait)
                    change.flags = libc::EV_DELETE;
                    nchanges = 1;
                    self.timer_armed = None;
                }
                _ => {}
            }
        }
        let r = unsafe { libc::kevent(self.poller, &change, nchanges, self.events.as_mut_ptr(), self.events.len() as i32, tsp) };
        if r <= 0 {
            return;
        }
        let start = out.len();
        for ev in &self.events[..r as usize] {
            let fd = ev.ident as i64;
            if fd == self.wake {
                *woke = true;
                continue;
            }
            if ev.flags & libc::EV_ERROR != 0 {
                continue;
            }
            #[cfg(target_os = "macos")]
            if ev.filter == libc::EVFILT_TIMER {
                self.timer_armed = None;
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
        for p in &pfds {
            if p.revents == 0 {
                continue;
            }
            let err = p.revents & (POLLERR | POLLHUP) as i16 != 0;
            out.push((p.fd as i64, err || p.revents & POLLRDNORM as i16 != 0, err || p.revents & POLLWRNORM as i16 != 0));
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
