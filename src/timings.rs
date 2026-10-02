//! Opt-in wall time and peak memory measurements for large apps.

use std::sync::LazyLock;
use std::time::Instant;

static ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("ROUNDHOUSE_TIMINGS").is_ok_and(|value| value == "1" || value == "true")
});

#[cfg(target_os = "macos")]
fn peak_rss_mb() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    Some(unsafe { usage.assume_init() }.ru_maxrss as u64 / (1024 * 1024))
}

#[cfg(target_os = "linux")]
fn peak_rss_mb() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peak_rss_mb() -> Option<u64> {
    None
}

/// Run a phase and print measurements only when requested.
pub fn phase<T>(name: impl std::fmt::Display, f: impl FnOnce() -> T) -> T {
    if !*ENABLED {
        return f();
    }
    let start = Instant::now();
    let output = f();
    match peak_rss_mb() {
        Some(mb) => eprintln!(
            "roundhouse-timing: {name}: {:.2}s (peak rss {mb} MB)",
            start.elapsed().as_secs_f64(),
        ),
        None => eprintln!(
            "roundhouse-timing: {name}: {:.2}s (peak rss unavailable)",
            start.elapsed().as_secs_f64(),
        ),
    }
    output
}
