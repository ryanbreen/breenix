//! Charges to a process's GPU memory quota (`docs/gui/gpu.md` sections 1.4 and 1.16).
//!
//! A charge is an object that releases itself when dropped, so it follows the memory it pays
//! for wherever that goes: a resource's record, a call's staging, or a submission whose
//! device requests outlive the call. The quota counter is shared by the process's charges and
//! outlives the process's record in the core, so memory still held after an exit or an exec
//! releases into the counter it was charged to. Releasing touches only atomics, so a charge
//! may drop under the core's lock.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::tracing::providers::gpu::{gauge, GPU_RESOURCE_BYTES};

use super::{with_core, EINVAL, ENODEV, ENOMEM, MEMORY_QUOTA};

/// One process's charged bytes.
pub(super) type Quota = Arc<AtomicU64>;

/// Bytes charged to one quota, released when dropped.
pub(super) struct QuotaCharge {
    quota: Quota,
    bytes: u64,
}

impl QuotaCharge {
    /// Charge `bytes` to `quota`, unless the quota cannot take them.
    pub(super) fn try_new(quota: &Quota, bytes: u64) -> Option<QuotaCharge> {
        quota
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                held.checked_add(bytes).filter(|&total| total <= MEMORY_QUOTA)
            })
            .ok()?;
        gauge(&GPU_RESOURCE_BYTES, bytes as i64);
        Some(QuotaCharge { quota: quota.clone(), bytes })
    }
}

impl Drop for QuotaCharge {
    fn drop(&mut self) {
        self.quota.fetch_sub(self.bytes, Ordering::AcqRel);
        gauge(&GPU_RESOURCE_BYTES, -(self.bytes as i64));
    }
}

/// Bytes charged to process `tgid`'s quota and its `RLIMIT_AS`, released when dropped: staging,
/// and `SUBMIT`'s private copies and device vertex storage.
pub(super) struct Charge {
    _quota: QuotaCharge,
    limits: Arc<crate::process::limits::Limits>,
    bytes: u64,
}

impl Charge {
    /// Charge `bytes` to process `tgid`; ENOMEM when its quota or `RLIMIT_AS` cannot take them.
    pub(super) fn new(tgid: u64, bytes: u64) -> Result<Charge, i64> {
        let quota = with_core(|c| c.proc_mut(tgid).quota.clone()).ok_or(ENODEV)?;
        let charged = QuotaCharge::try_new(&quota, bytes).ok_or(ENOMEM)?;
        let limits = crate::process::with_process_manager(|m| {
            let p = m.get_process(crate::process::ProcessId::new(tgid)).ok_or(EINVAL)?;
            if !p.reserve_gpu_bytes(bytes) {
                return Err(ENOMEM);
            }
            Ok(p.limits.clone())
        })
        .unwrap_or(Err(EINVAL))?;
        Ok(Charge { _quota: charged, limits, bytes })
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.limits.release_gpu_bytes(self.bytes);
    }
}
