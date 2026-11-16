//! Root-owned block nodes used as mount sources. Node opens have no I/O API.

use super::DeviceType;
use crate::fs::attrs::InodeAttrs;

// Separate from the character devices and the shm/mqueue directory inodes.
pub(super) const INODE_BASE: u64 = 0x2000;

const fn disk_names() -> [[u8; 7]; 256] {
    let mut names = [*b"disk000"; 256];
    let mut n = 0;
    while n < names.len() {
        if n >= 100 {
            names[n][4] = b'0' + (n / 100) as u8;
            names[n][5] = b'0' + (n / 10 % 10) as u8;
            names[n][6] = b'0' + (n % 10) as u8;
        } else if n >= 10 {
            names[n][4] = b'0' + (n / 10) as u8;
            names[n][5] = b'0' + (n % 10) as u8;
        } else {
            names[n][4] = b'0' + n as u8;
        }
        n += 1;
    }
    names
}

static NAMES: [[u8; 7]; 256] = disk_names();

pub fn name(n: u8) -> &'static str {
    let len = if n >= 100 {
        7
    } else if n >= 10 {
        6
    } else {
        5
    };
    core::str::from_utf8(&NAMES[usize::from(n)][..len]).expect("decimal disk name")
}

pub fn attrs() -> InodeAttrs {
    InodeAttrs::new(0o060600, 0, 0)
}

/// Called after the node's permission check, before allocating a descriptor.
pub fn open_refusal(device: &DeviceType) -> Option<u64> {
    match device {
        DeviceType::Disk(_) => Some(crate::syscall::errno::ENXIO as u64),
        _ => None,
    }
}

// The metadata above is also compiled directly by the host check.
#[cfg(not(test))]
mod hardware {
    use super::{name, DeviceType, INODE_BASE};
    use crate::block::BlockDevice;
    use crate::fs::devfs::DeviceNode;
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    const MAJOR: u32 = 8;

    #[cfg(target_arch = "x86_64")]
    fn devices() -> Vec<Box<dyn BlockDevice>> {
        crate::block::devices()
    }

    #[cfg(target_arch = "aarch64")]
    fn devices() -> Vec<Box<dyn BlockDevice>> {
        let mut devices: Vec<Box<dyn BlockDevice>> = Vec::new();
        for index in 0..crate::drivers::virtio::block_mmio::MAX_BLOCK_DEVICES {
            if let Some(device) = crate::block::virtio::VirtioBlockWrapper::new(index) {
                devices.push(Box::new(device));
            }
        }
        for index in 0..crate::drivers::ahci::sata_device_count() {
            if let Some(device) = crate::drivers::ahci::get_block_device_by_index(index) {
                devices.push(Box::new(device));
            }
        }
        devices
    }

    /// Obtain the disk named by /dev/disk<N> for mount(2).
    pub fn disk_device(n: usize) -> Option<Box<dyn BlockDevice>> {
        if n > usize::from(u8::MAX) {
            return None;
        }
        devices().into_iter().nth(n)
    }

    fn node(n: u8) -> DeviceNode {
        DeviceNode::new(DeviceType::Disk(n), MAJOR, u32::from(n))
    }

    // Discover on lookup: devfs can initialize before the storage drivers.
    pub fn nodes() -> impl ExactSizeIterator<Item = DeviceNode> {
        (0..devices().len().min(256)).map(|n| node(n as u8))
    }

    pub fn lookup(path: &str) -> Option<DeviceNode> {
        let n: u8 = path.strip_prefix("disk")?.parse().ok()?;
        if path != name(n) {
            return None;
        }
        disk_device(usize::from(n))?;
        Some(node(n))
    }

    pub fn lookup_by_inode(inode: u64) -> Option<DeviceNode> {
        let n = u8::try_from(inode.checked_sub(INODE_BASE)?).ok()?;
        disk_device(usize::from(n))?;
        Some(node(n))
    }

    pub fn lookup_by_rdev(rdev: u64) -> Option<DeviceNode> {
        if rdev >> 8 != u64::from(MAJOR) {
            return None;
        }
        let n = (rdev & 0xff) as u8;
        disk_device(usize::from(n))?;
        Some(node(n))
    }
}

#[cfg(not(test))]
pub use hardware::{disk_device, lookup, lookup_by_inode, lookup_by_rdev, nodes};
