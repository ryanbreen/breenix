//! Hardware disk writes on physical x86 boots (docs/metal/README.md, "The write fence").
//!
//! Hardware disks are read-only until the boot's partitions are chosen. Then the fence names one disk and up to two
//! LBA ranges on it (the root partition, and the home partition when the boot has one), and a driver accepts a write
//! only when its first and last sector lie inside one of those ranges. The fence is set before root mounts and
//! sealed once the mounts are done; a recovery install sets it to the test root partition for its copy and clears
//! it afterwards.

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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HardwareDisk {
    Ahci(usize),
    Virtio(usize),
    Nvme(usize),
}

impl core::fmt::Display for HardwareDisk {
    fn fmt(&self, out: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            HardwareDisk::Ahci(port) => write!(out, "AHCI port {}", port),
            HardwareDisk::Virtio(_) => out.write_str("VirtIO disk"),
            HardwareDisk::Nvme(_) => out.write_str("NVMe disk"),
        }
    }
}

// The fence exists on physical x86 boots; host tests compile it too.
#[cfg(any(target_arch = "x86_64", test))]
const RANGES: usize = 2;

/// The disk writes may reach and the inclusive LBA ranges on it.
#[cfg(any(target_arch = "x86_64", test))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fence {
    pub disk: Option<HardwareDisk>,
    pub ranges: [(u64, u64); RANGES],
    pub count: usize,
    pub sealed: bool,
}

#[cfg(any(target_arch = "x86_64", test))]
impl Fence {
    pub const CLOSED: Fence = Fence { disk: None, ranges: [(0, 0); RANGES], count: 0, sealed: false };

    /// Adds a range on `disk`; refused once sealed, for a second disk, or past the range limit.
    pub fn add(&mut self, disk: HardwareDisk, first: u64, last: u64) -> Result<(), &'static str> {
        if self.sealed {
            return Err("the write fence is sealed");
        }
        if self.disk.is_some_and(|fenced| fenced != disk) {
            return Err("the write fence already names another disk");
        }
        if self.count == RANGES || first > last {
            return Err("the write fence takes two ranges");
        }
        self.disk = Some(disk);
        self.ranges[self.count] = (first, last);
        self.count += 1;
        Ok(())
    }

    /// Whether a write of `count` sectors from `lba` on `disk` lies inside one range.
    pub fn allows(&self, disk: HardwareDisk, lba: u64, count: usize) -> bool {
        let Some(end) = (count as u64).checked_sub(1).and_then(|span| lba.checked_add(span)) else {
            return count == 0 && self.disk == Some(disk);
        };
        self.disk == Some(disk) && self.ranges[..self.count].iter().any(|&(first, last)| lba >= first && end <= last)
    }

    pub fn ranges(&self) -> &[(u64, u64)] {
        &self.ranges[..self.count]
    }
}

#[cfg(target_arch = "x86_64")]
static FENCE: spin::Mutex<Fence> = spin::Mutex::new(Fence::CLOSED);

/// Writes refused by the fence, and how many of them have been reported.
#[cfg(target_arch = "x86_64")]
static REFUSED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
const REPORTED_REFUSALS: u64 = 8;

/// Whether `disk` takes no writes at all: a physical boot whose fence does not name it.
pub fn hardware_disk_read_only(disk: HardwareDisk) -> bool {
    #[cfg(target_arch = "x86_64")]
    { hardware_read_only() && FENCE.lock().disk != Some(disk) }
    #[cfg(not(target_arch = "x86_64"))]
    { let _ = disk; false }
}

/// Whether a driver may write `count` sectors from `lba` on `disk`; a refusal is counted and the first ones reported.
pub fn hardware_write_allowed(disk: HardwareDisk, lba: u64, count: usize) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if !hardware_read_only() || FENCE.lock().allows(disk, lba, count) {
            return true;
        }
        let refused = REFUSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
        if refused <= REPORTED_REFUSALS {
            log::warn!(
                "[block] write refused by the fence: LBA {}+{} on {}, outside the boot's partitions ({} refused)",
                lba,
                count,
                disk,
                refused
            );
        }
        false
    }
    #[cfg(not(target_arch = "x86_64"))]
    { let _ = (disk, lba, count); true }
}

/// Adds a partition's range to the fence.
#[cfg(target_arch = "x86_64")]
pub(super) fn fence_add(disk: HardwareDisk, first: u64, last: u64) -> Result<(), &'static str> {
    FENCE.lock().add(disk, first, last)
}

/// Seals the fence once the boot's mounts are done and reports it.
#[cfg(target_arch = "x86_64")]
pub fn fence_seal() {
    let mut fence = FENCE.lock();
    fence.sealed = true;
    let fence = *fence;
    if let Some(disk) = fence.disk {
        match fence.ranges() {
            [(a, b)] => log::info!("[block] write fence: {} LBA {}-{}", disk, a, b),
            [(a, b), (c, d)] => log::info!("[block] write fence: {} LBA {}-{}, {}-{}", disk, a, b, c, d),
            _ => {}
        }
    } else if hardware_read_only() {
        log::info!("[block] write fence: no partition chosen; hardware disks stay read-only");
    }
}

/// Clears an unsealed fence (a recovery install's copy is over).
#[cfg(target_arch = "x86_64")]
pub(super) fn fence_clear() {
    let mut fence = FENCE.lock();
    if !fence.sealed {
        *fence = Fence::CLOSED;
    }
}

/// The sealed fence, for the fault-injection exercise.
#[cfg(target_arch = "x86_64")]
pub fn fence() -> Fence {
    *FENCE.lock()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_must_lie_inside_one_range_of_the_fenced_disk() {
        let root = HardwareDisk::Ahci(0);
        let mut fence = Fence::CLOSED;
        assert!(!fence.allows(root, 100, 1));
        fence.add(root, 100, 199).unwrap();
        fence.add(root, 300, 399).unwrap();
        assert!(fence.allows(root, 100, 1));
        assert!(fence.allows(root, 199, 1));
        assert!(fence.allows(root, 300, 100));
        assert!(!fence.allows(root, 99, 1));
        assert!(!fence.allows(root, 199, 2));
        assert!(!fence.allows(root, 250, 1));
        assert!(!fence.allows(root, 390, 20));
        assert!(!fence.allows(root, u64::MAX, 2));
        assert!(!fence.allows(HardwareDisk::Ahci(1), 150, 1));
        assert!(fence.add(root, 500, 599).is_err());
        let mut other = Fence::CLOSED;
        other.add(root, 1, 2).unwrap();
        assert!(other.add(HardwareDisk::Ahci(1), 1, 2).is_err());
        other.sealed = true;
        assert!(Fence { count: 0, disk: None, ..other }.add(root, 1, 2).is_err());
    }
}
