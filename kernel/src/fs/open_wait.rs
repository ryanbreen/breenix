//! Throwaway #2945 diagnostic branch; no wait policy changes.
pub mod counters;
pub use counters::Metric;
use counters::{Counters, METRICS};

const SLOTS: usize = 128;
static COUNTERS: [Counters; SLOTS] = [const { Counters::new() }; SLOTS];

fn tid() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::percpu::Aarch64PerCpu;
        use crate::arch_impl::traits::PerCpuOps;
        let ptr = Aarch64PerCpu::current_thread_ptr() as *const crate::task::thread::Thread;
        if ptr.is_null() { 0 } else { unsafe { (*ptr).id() } }
    }
    #[cfg(target_arch = "x86_64")]
    { crate::per_cpu::current_thread().map_or(0, |thread| thread.id()) }
}
fn now() -> u64 {
    let (s, ns) = crate::time::get_monotonic_time_ns();
    s.saturating_mul(1_000_000_000).saturating_add(ns)
}
fn slot(tid: u64) -> &'static Counters { &COUNTERS[tid as usize % SLOTS] }

pub fn add(metric: Metric, value: u64) {
    let tid = tid();
    slot(tid).add(tid, metric, value);
}
pub struct Stage { tid: u64, start: u64, metric: Metric }
pub fn stage(metric: Metric) -> Stage { Stage { tid: tid(), start: now(), metric } }
impl Drop for Stage {
    fn drop(&mut self) { slot(self.tid).add(self.tid, self.metric, now().saturating_sub(self.start)); }
}

pub struct Open<'a> { tid: u64, start: u64, owned: bool, path: &'a [u8] }
pub fn begin(path: &[u8]) -> Open<'_> {
    let tid = tid();
    Open { tid, start: now(), owned: slot(tid).begin(tid), path }
}
impl Drop for Open<'_> {
    fn drop(&mut self) {
        let end = now();
        let elapsed = end.saturating_sub(self.start);
        let values = if self.owned { slot(self.tid).finish(self.tid) } else { None };
        if elapsed < 100_000_000 { return; }
        let valid = values.is_some();
        let v = values.unwrap_or([0; METRICS]);
        crate::serial_println!(
            "OPEN_WAIT_DIAG tid={} path={} start_ns={} end_ns={} elapsed_ns={} valid={} namei_ns={} read_guard_ns={} write_guard_ns={} create_ns={} vfs_ns={} mmio_ns={} reclaim_ns={} requests={} expired={} expired_done={}",
            self.tid, core::str::from_utf8(self.path).unwrap_or("<bytes>"), self.start, end, elapsed,
            valid, v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9]
        );
    }
}
