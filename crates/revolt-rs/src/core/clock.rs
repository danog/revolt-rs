//! The loop clock: hrtime(true) / 1e9 as Revolt's now(), from the same monotonic clock PHP's
//! hrtime() reads (CLOCK_MONOTONIC on Linux, CLOCK_UPTIME_RAW on macOS, the performance counter
//! on Windows).

#[cfg(target_os = "macos")]
const CLOCK: libc::clockid_t = libc::CLOCK_UPTIME_RAW;
#[cfg(all(unix, not(target_os = "macos")))]
const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

/// Seconds (with nanoseconds) on the monotonic clock.
#[cfg(unix)]
#[inline]
pub fn now() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(CLOCK, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

#[cfg(windows)]
#[inline]
pub fn now() -> f64 {
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        QueryPerformanceCounter(&mut c);
        QueryPerformanceFrequency(&mut f);
    }
    (c / f) as f64 + (c % f) as f64 / f as f64
}
