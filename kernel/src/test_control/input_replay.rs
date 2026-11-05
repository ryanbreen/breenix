//! Observation of one completion replay, including its release report.
use super::{enabled, take, POINT_TABLE};
use crate::syscall::errno::ENOSYS;
use core::sync::atomic::{AtomicBool, Ordering};
// A replay transaction includes its release report. Suppression fires once and
// keeps that transaction silent while both completions still decode and requeue.
static INPUT_REPLAY_THREAD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static INPUT_REPLAY_DEVICE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static INPUT_REPLAY_CALLS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static INPUT_REPLAY_SILENT: AtomicBool = AtomicBool::new(false);
pub(crate) struct InputReplay;
impl InputReplay {
    pub(crate) fn begin(device: u32, suppress: bool) -> Result<Self, u64> {
        if !enabled() { return Err(ENOSYS as u64); }
        let tid = crate::task::scheduler::current_thread_id().ok_or(crate::syscall::errno::ESRCH as u64)?;
        INPUT_REPLAY_THREAD.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| crate::syscall::errno::EBUSY as u64)?;
        INPUT_REPLAY_CALLS.store(0, Ordering::Relaxed);
        INPUT_REPLAY_SILENT.store(false, Ordering::Relaxed);
        INPUT_REPLAY_DEVICE.store(device, Ordering::Release);
        if suppress {
            crate::arch_without_interrupts(|| POINT_TABLE[breengel_abi::tc_point::INPUT_REPORT_SKIPPED as usize].arm(device as u64, 0));
        }
        Ok(Self)
    }
    pub(crate) fn calls(&self) -> u32 { INPUT_REPLAY_CALLS.load(Ordering::Relaxed) }
}
impl Drop for InputReplay {
    fn drop(&mut self) {
        crate::arch_without_interrupts(|| POINT_TABLE[breengel_abi::tc_point::INPUT_REPORT_SKIPPED as usize].disarm());
        INPUT_REPLAY_DEVICE.store(0, Ordering::Relaxed);
        INPUT_REPLAY_THREAD.store(0, Ordering::Release);
    }
}
pub(crate) fn input_report_allowed(device: u32) -> bool {
    if !enabled() { return true; }
    crate::arch_without_interrupts(|| {
        let observed = INPUT_REPLAY_DEVICE.load(Ordering::Acquire);
        if observed != 0 {
            if observed != device || crate::task::scheduler::current_thread_id()
                != Some(INPUT_REPLAY_THREAD.load(Ordering::Acquire)) {
                return true;
            }
            if take(breengel_abi::tc_point::INPUT_REPORT_SKIPPED, device as u64).is_some() {
                INPUT_REPLAY_SILENT.store(true, Ordering::Relaxed);
            }
            if INPUT_REPLAY_SILENT.load(Ordering::Relaxed) { return false; }
            INPUT_REPLAY_CALLS.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            take(breengel_abi::tc_point::INPUT_REPORT_SKIPPED, device as u64).is_none()
        }
    })
}
