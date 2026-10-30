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

/// One process's charged bytes: everything the quota limits, and the part of it that is
/// `SUBMIT`'s copies and vertex storage (`COUNTERS.copy_bytes`; the rest is `resource_bytes`).
#[derive(Default)]
pub(super) struct Counts {
    pub(super) held: AtomicU64,
    pub(super) copies: AtomicU64,
    resources: AtomicU64,
}

impl Counts {
    /// Read this class directly: held and copies can change independently while
    /// a charge drops outside the core lock, so their difference is not a snapshot.
    pub(super) fn resource_bytes(&self) -> u64 {
        self.resources.load(Ordering::Acquire)
    }
}

pub(super) type Quota = Arc<Counts>;

/// Bytes charged to one quota, released when dropped.
pub(super) struct QuotaCharge {
    quota: Quota,
    bytes: u64,
    copy: bool,
}

impl QuotaCharge {
    /// Charge `bytes` to `quota`, unless the quota cannot take them; `copy` for `SUBMIT`'s
    /// copies and vertex storage.
    pub(super) fn try_new(quota: &Quota, bytes: u64, copy: bool) -> Option<QuotaCharge> {
        quota
            .held
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                held.checked_add(bytes).filter(|&total| total <= MEMORY_QUOTA)
            })
            .ok()?;
        if copy {
            quota.copies.fetch_add(bytes, Ordering::AcqRel);
        } else {
            quota.resources.fetch_add(bytes, Ordering::AcqRel);
        }
        gauge(&GPU_RESOURCE_BYTES, bytes as i64);
        Some(QuotaCharge { quota: quota.clone(), bytes, copy })
    }
}

impl Drop for QuotaCharge {
    fn drop(&mut self) {
        if self.copy {
            self.quota.copies.fetch_sub(self.bytes, Ordering::AcqRel);
        } else {
            self.quota.resources.fetch_sub(self.bytes, Ordering::AcqRel);
        }
        self.quota.held.fetch_sub(self.bytes, Ordering::AcqRel);
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
    /// `copy` for `SUBMIT`'s copies and vertex storage, reported apart from staging.
    pub(super) fn new(tgid: u64, bytes: u64, copy: bool) -> Result<Charge, i64> {
        let quota = with_core(|c| c.proc_mut(tgid).quota.clone()).ok_or(ENODEV)?;
        let charged = QuotaCharge::try_new(&quota, bytes, copy).ok_or(ENOMEM)?;
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
