//! Minimal non-GPU userspace heartbeat for scheduler liveness diagnosis.

use libbreenix::process::gettid;
use libbreenix::time::{now_monotonic, sleep_ms};

fn monotonic_ms() -> u64 {
    now_monotonic()
        .map(|ts| ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000)
        .unwrap_or(0)
}

fn main() {
    let tid = gettid().map(|tid| tid.raw()).unwrap_or(0);

    loop {
        print!("[heartbeat] tid={} uptime_ms={}\n", tid, monotonic_ms());
        let _ = sleep_ms(1_000);
    }
}
