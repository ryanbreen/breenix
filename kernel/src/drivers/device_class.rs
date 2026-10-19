//! Device-class lines.
//!
//! The boot-path devices milestone (`docs/boot-path.json`) states what the
//! kernel does with its devices, not which device it does it with: a storage
//! device is found, initialized and read, and a network device is found. Each
//! driver reports those events here, and these are the lines the milestone's
//! stages match, whether the device is QEMU's VirtIO or E1000, Parallels' or
//! VMware's, or a physical machine's AHCI disk and Broadcom NIC.
//!
//! - found: bus enumeration saw a device of the class (a PCI class code, a
//!   VirtIO MMIO device ID, or a platform controller whose registers answer).
//! - storage initialized: a storage driver attached at least one disk.
//! - storage read: a block read through `BlockDevice` completed.

use core::fmt;

/// The class a device belongs to.
#[derive(Clone, Copy)]
pub enum Class {
    Storage,
    Network,
}

fn line(event: &str, args: fmt::Arguments) {
    #[cfg(target_arch = "x86_64")]
    log::info!("[devices] {}: {}", event, args);
    #[cfg(target_arch = "aarch64")]
    crate::serial_println!("[devices] {}: {}", event, args);
}

#[cfg(target_arch = "aarch64")]
fn screen(stage: crate::console::splash::Stage) {
    crate::console::splash::stage(stage);
}

/// A device of `class` was found; `args` says where and what it is.
pub fn found(class: Class, args: fmt::Arguments) {
    match class {
        Class::Storage => {
            line("storage found", args);
            #[cfg(target_arch = "aarch64")]
            screen(crate::console::splash::Stage::StorageFound);
        }
        Class::Network => {
            line("network found", args);
            #[cfg(target_arch = "aarch64")]
            screen(crate::console::splash::Stage::NetworkFound);
        }
    }
}

/// `driver` attached `disks` storage disks. Returns without a line when `disks` is 0.
pub fn storage_initialized(driver: &str, disks: usize) {
    if disks == 0 {
        return;
    }
    line(
        "storage initialized",
        format_args!("{driver}, {disks} disk(s)"),
    );
    #[cfg(target_arch = "aarch64")]
    screen(crate::console::splash::Stage::StorageInitialized);
}

/// Report the root filesystem's completed superblock read without issuing extra I/O.
pub fn storage_read() {
    line("storage read", format_args!("root ext2 superblock"));
    #[cfg(target_arch = "aarch64")]
    screen(crate::console::splash::Stage::StorageRead);
}
