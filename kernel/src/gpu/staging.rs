use super::{with_core, Pages, EINVAL, ENODEV, ENOMEM, MEMORY_QUOTA, PAGE};

/// A readback's staged rectangle: whole frames charged to the process's GPU quota (the
/// guest-backing quota and `RLIMIT_AS`, as resource backing is), rather than heap pixel buffers.
/// Frame descriptors remain heap metadata. The staging is CPU-only and freed with the call.
pub(super) struct Staging {
    pub(super) pages: Pages,
    tgid: u64,
    charge: u64,
}

impl Staging {
    pub(super) fn new(tgid: u64, bytes: usize) -> Result<Staging, i64> {
        let frames = bytes.div_ceil(PAGE);
        let charge = (frames * PAGE) as u64;
        with_core(|c| {
            let p = c.proc_mut(tgid);
            if p.bytes.saturating_add(charge) > MEMORY_QUOTA {
                return Err(ENOMEM);
            }
            p.bytes += charge;
            Ok(())
        })
        .ok_or(ENODEV)??;
        let uncharge = || {
            with_core(|c| {
                if let Some(p) = c.procs.get_mut(&tgid) {
                    p.bytes = p.bytes.saturating_sub(charge);
                }
            });
        };
        let limits = crate::process::with_process_manager(|m| {
            let p = m.get_process(crate::process::ProcessId::new(tgid)).ok_or(EINVAL)?;
            if !p.reserve_gpu_bytes(charge) {
                return Err(ENOMEM);
            }
            Ok(p.limits.clone())
        })
        .unwrap_or(Err(EINVAL));
        let limits = match limits {
            Ok(l) => l,
            Err(e) => {
                uncharge();
                return Err(e);
            }
        };
        let Some(mut pages) = Pages::new(frames) else {
            limits.release_gpu_bytes(charge);
            uncharge();
            return Err(ENOMEM);
        };
        pages.charge_to(limits);
        Ok(Staging { pages, tgid, charge })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let (tgid, charge) = (self.tgid, self.charge);
        with_core(|c| {
            if let Some(p) = c.procs.get_mut(&tgid) {
                p.bytes = p.bytes.saturating_sub(charge);
            }
        });
    }
}

