//! onSignal() without ext-pcntl: a sigaction handler (async-signal-safe: atomics and write(2)
//! only) counts each signal process-wide and wakes every loop watching signals through its wake
//! pipe; a loop compares the counters with the values it saw last. The disposition a signal had
//! before its first watcher is restored when its last watcher goes (process-wide refcounts:
//! loops of several ZTS threads may watch the same signal).

#[cfg(unix)]
mod imp {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
    use std::sync::Mutex;

    const NSIG: usize = 65;
    const SLOTS: usize = 64;

    static COUNTERS: [AtomicU64; NSIG] = [const { AtomicU64::new(0) }; NSIG];
    static WAKE: [AtomicI32; SLOTS] = [const { AtomicI32::new(-1) }; SLOTS];

    struct Installed {
        refs: u32,
        old: libc::sigaction,
    }

    static INSTALLED: Mutex<Vec<Option<Installed>>> = Mutex::new(Vec::new());

    extern "C" fn handler(sig: libc::c_int) {
        unsafe {
            #[cfg(target_os = "linux")]
            let errno = libc::__errno_location();
            #[cfg(not(target_os = "linux"))]
            let errno = libc::__error();
            let saved = *errno;
            if (sig as usize) < NSIG {
                COUNTERS[sig as usize].fetch_add(1, Ordering::SeqCst);
            }
            for w in WAKE.iter() {
                let fd = w.load(Ordering::SeqCst);
                if fd >= 0 {
                    let b = 1u8;
                    libc::write(fd, &b as *const u8 as *const _, 1);
                }
            }
            *errno = saved;
        }
    }

    /// Installs the handler for `sig` (refcounted); Err(errno) when the signal cannot be caught.
    fn install(sig: i32) -> Result<(), i32> {
        if sig <= 0 || sig as usize >= NSIG {
            return Err(libc::EINVAL);
        }
        let mut t = INSTALLED.lock().unwrap();
        if t.len() < NSIG {
            t.resize_with(NSIG, || None);
        }
        if let Some(i) = &mut t[sig as usize] {
            i.refs += 1;
            return Ok(());
        }
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handler as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigfillset(&mut sa.sa_mask);
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(sig, &sa, &mut old) != 0 {
                return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EINVAL));
            }
            t[sig as usize] = Some(Installed { refs: 1, old });
        }
        Ok(())
    }

    fn uninstall(sig: i32) {
        let mut t = INSTALLED.lock().unwrap();
        let Some(slot) = t.get_mut(sig as usize) else { return };
        let Some(i) = slot else { return };
        i.refs -= 1;
        if i.refs == 0 {
            unsafe { libc::sigaction(sig, &i.old, std::ptr::null_mut()) };
            *slot = None;
        }
    }

    /// The signal watchers of one loop.
    #[derive(Default)]
    pub struct Watchers {
        /// signal => callback ids, in activation order
        pub by_signal: HashMap<i32, Vec<u64>>,
        seen: HashMap<i32, u64>,
        pipe: Option<(i32, i32)>,
        slot: Option<usize>,
        fork_gen: u64,
    }

    impl Watchers {
        /// Watches `sig` for callback `id`; Err((errno, message)) when it cannot be.
        pub fn add(&mut self, sig: i32, id: u64) -> Result<(), (i32, String)> {
            self.ensure_pipe().map_err(|e| (e, std::io::Error::from_raw_os_error(e).to_string()))?;
            if !self.by_signal.contains_key(&sig) {
                install(sig).map_err(|e| (e, std::io::Error::from_raw_os_error(e).to_string()))?;
                self.seen.insert(sig, COUNTERS[sig as usize].load(Ordering::SeqCst));
            }
            self.by_signal.entry(sig).or_default().push(id);
            Ok(())
        }

        pub fn remove(&mut self, sig: i32, id: u64) {
            let Some(v) = self.by_signal.get_mut(&sig) else { return };
            if let Some(p) = v.iter().position(|&x| x == id) {
                v.remove(p);
            }
            if v.is_empty() {
                self.by_signal.remove(&sig);
                self.seen.remove(&sig);
                uninstall(sig);
            }
            if self.by_signal.is_empty() {
                self.close_pipe();
            }
        }

        /// The read end of the wake pipe (-1: none).
        pub fn wake_fd(&self) -> i64 {
            self.pipe.map_or(-1, |p| p.0 as i64)
        }

        /// The signals delivered since the last call (each once), in signal order; drains the pipe.
        pub fn take_delivered(&mut self) -> Vec<i32> {
            let mut out = Vec::new();
            if self.by_signal.is_empty() {
                return out;
            }
            if let Some((r, _)) = self.pipe {
                let mut buf = [0u8; 64];
                while unsafe { libc::read(r, buf.as_mut_ptr() as *mut _, buf.len()) } > 0 {}
            }
            let mut sigs: Vec<i32> = self.seen.keys().copied().collect();
            sigs.sort_unstable();
            for s in sigs {
                let c = COUNTERS[s as usize].load(Ordering::SeqCst);
                let seen = self.seen.get_mut(&s).unwrap();
                if c != *seen {
                    *seen = c;
                    out.push(s);
                }
            }
            out
        }

        fn ensure_pipe(&mut self) -> Result<(), i32> {
            let g = super::super::backend::FORK_GEN.load(Ordering::Relaxed);
            if self.pipe.is_some() && self.fork_gen != g {
                // a fork() child: the pipe is shared with the parent
                self.close_pipe();
            }
            if self.pipe.is_some() {
                return Ok(());
            }
            let mut fds = [0i32; 2];
            unsafe {
                if libc::pipe(fds.as_mut_ptr()) != 0 {
                    return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EMFILE));
                }
                for fd in fds {
                    libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
            let slot = WAKE.iter().position(|w| w.compare_exchange(-1, fds[1], Ordering::SeqCst, Ordering::SeqCst).is_ok());
            if slot.is_none() {
                unsafe {
                    libc::close(fds[0]);
                    libc::close(fds[1]);
                }
                return Err(libc::EMFILE);
            }
            self.slot = slot;
            self.pipe = Some((fds[0], fds[1]));
            self.fork_gen = g;
            Ok(())
        }

        fn close_pipe(&mut self) {
            if let Some(s) = self.slot.take() {
                WAKE[s].store(-1, Ordering::SeqCst);
            }
            if let Some((r, w)) = self.pipe.take() {
                unsafe {
                    libc::close(r);
                    libc::close(w);
                }
            }
        }
    }

    impl Drop for Watchers {
        fn drop(&mut self) {
            for (&sig, _) in self.by_signal.iter() {
                uninstall(sig);
            }
            self.by_signal.clear();
            self.close_pipe();
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;

    /// Windows: no signals (onSignal() throws UnsupportedFeatureException before this is used).
    #[derive(Default)]
    pub struct Watchers {
        pub by_signal: HashMap<i32, Vec<u64>>,
    }

    impl Watchers {
        pub fn add(&mut self, _sig: i32, _id: u64) -> Result<(), (i32, String)> {
            Err((0, "Signal handling is not supported on Windows".into()))
        }
        pub fn remove(&mut self, _sig: i32, _id: u64) {}
        pub fn wake_fd(&self) -> i64 {
            -1
        }
        pub fn take_delivered(&mut self) -> Vec<i32> {
            Vec::new()
        }
    }
}

pub use imp::Watchers;

/// Whether onSignal() is supported at all.
pub const SUPPORTED: bool = cfg!(unix);
