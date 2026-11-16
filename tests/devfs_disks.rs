//! Disk names and metadata shared with devfs, without hardware or syscall shims.
extern crate alloc;

#[path = "../kernel/src/fs/attrs.rs"]
pub mod attrs;
pub mod fs {
    pub use crate::attrs;
}
#[path = "../kernel/src/syscall/errno.rs"]
pub mod errno;
pub mod syscall {
    pub use crate::errno;
}
#[path = "../kernel/src/fs/devfs/device_type.rs"]
mod device_type;
use device_type::DeviceType;
#[path = "../kernel/src/fs/devfs/disks.rs"]
mod disks;

fn existing_devices() -> Vec<DeviceType> {
    use DeviceType::*;
    let mut devices = vec![Null, InputEvents, Uinput, Ufs, Kmsg, ScreenEvidence,
        Zero, Console, Tty, Rtc, Ptmx, Tty0];
    devices.extend((1..=6).map(Vt));
    devices.extend((0..=6).map(Vcs));
    devices.extend((0..=6).map(Vcsa));
    devices
}

#[test]
fn disk_names_and_root_only_block_attributes_have_distinct_inodes() {
    let existing = existing_devices();
    let mut inodes = std::collections::BTreeSet::new();
    for n in 0..=u8::MAX {
        assert_eq!(disks::name(n), format!("disk{n}"));
        let attrs = disks::attrs();
        assert_eq!((attrs.mode, attrs.uid, attrs.gid), (0o060600, 0, 0));
        let inode = DeviceType::Disk(n).inode();
        assert!(inodes.insert(inode), "duplicate disk inode {inode}");
        for device in &existing {
            assert_ne!(inode, device.inode(), "disk{n} aliases {device:?}");
        }
        for directory in [1, 43, 0x1000, 0x1001] {
            assert_ne!(inode, directory);
        }
    }
}

#[test]
fn open_refuses_disks_with_enxio_and_leaves_existing_devices_to_their_handlers() {
    for n in 0..=u8::MAX {
        assert_eq!(disks::open_refusal(&DeviceType::Disk(n)), Some(errno::ENXIO as u64));
    }
    for device in existing_devices() {
        assert_eq!(disks::open_refusal(&device), None, "refused {device:?}");
    }
}
