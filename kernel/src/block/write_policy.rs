// Fail closed until early boot identifies the platform, before any driver starts.
#[cfg(target_arch = "x86_64")]
static HARDWARE_READ_ONLY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

#[cfg(target_arch = "x86_64")]
pub fn init_hardware_policy(qemu: bool) {
    HARDWARE_READ_ONLY.store(!qemu, core::sync::atomic::Ordering::Release);
}

/// Hardware writes are reserved for the storage lane on physical x86 boots.
/// RAM-disk writes do not pass through hardware drivers.
pub fn hardware_read_only() -> bool {
    #[cfg(target_arch = "x86_64")]
    { HARDWARE_READ_ONLY.load(core::sync::atomic::Ordering::Acquire) }
    #[cfg(not(target_arch = "x86_64"))]
    { false }
}

/// A stable backend identity, shared by wrappers and their hardware driver.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HardwareDisk {
    Ahci(usize),
    Virtio(usize),
    Nvme(usize),
}

#[cfg(target_arch = "x86_64")]
static WRITE_GRANT: spin::Mutex<Option<HardwareDisk>> = spin::Mutex::new(None);

/// Keep the physical-boot policy intact; grant only the selected backend.
pub fn hardware_disk_read_only(disk: HardwareDisk) -> bool {
    #[cfg(target_arch = "x86_64")]
    { hardware_read_only() && *WRITE_GRANT.lock() != Some(disk) }
    #[cfg(not(target_arch = "x86_64"))]
    { let _ = disk; false }
}

#[cfg(target_arch = "x86_64")]
pub(super) fn hardware_writes(disk: HardwareDisk, granted: bool) {
    *WRITE_GRANT.lock() = if granted { Some(disk) } else { None };
}


#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn grant_only_opens_the_selected_backend_and_keeps_physical_policy() {
        init_hardware_policy(false);
        let root = HardwareDisk::Ahci(1);
        hardware_writes(root, true);
        assert!(hardware_read_only());
        assert!(!hardware_disk_read_only(root));
        for other in [HardwareDisk::Ahci(0), HardwareDisk::Virtio(1), HardwareDisk::Nvme(1)] {
            assert!(hardware_disk_read_only(other));
        }
        hardware_writes(root, false);
        assert!(hardware_disk_read_only(root));
    }
}
