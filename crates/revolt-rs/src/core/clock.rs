//! The loop clock: hrtime(true) / 1e9 as Revolt's now(), from the same monotonic clock PHP's
//! hrtime() reads (CLOCK_MONOTONIC on Linux, CLOCK_UPTIME_RAW on macOS, the performance counter
//! on Windows).

#[cfg(target_os = "macos")]
const CLOCK: libc::clockid_t = libc::CLOCK_UPTIME_RAW;
#[cfg(all(unix, not(target_os = "macos")))]
const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

#[cfg(target_os = "macos")]
extern "C" {
    fn clock_gettime_nsec_np(clock: libc::clockid_t) -> u64;
}

/// Seconds (with nanoseconds) on the monotonic clock: macOS reads its nanoseconds directly (what
/// clock_gettime() and PHP's hrtime() read too).
#[cfg(target_os = "macos")]
#[inline]
pub fn now() -> f64 {
    unsafe { clock_gettime_nsec_np(CLOCK) as f64 / 1e9 }
}

/// Seconds (with nanoseconds) on the monotonic clock.
#[cfg(all(unix, not(target_os = "macos")))]
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
