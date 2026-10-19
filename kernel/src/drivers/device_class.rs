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
fn screen(stage: crate::graphics::boot_screen::Stage) {
    crate::graphics::boot_screen::stage(stage);
}

/// A device of `class` was found; `args` says where and what it is.
pub fn found(class: Class, args: fmt::Arguments) {
    match class {
        Class::Storage => {
            line("storage found", args);
            #[cfg(target_arch = "aarch64")]
            screen(crate::graphics::boot_screen::Stage::StorageFound);
        }
        Class::Network => {
            line("network found", args);
            #[cfg(target_arch = "aarch64")]
            screen(crate::graphics::boot_screen::Stage::NetworkFound);
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
    screen(crate::graphics::boot_screen::Stage::StorageInitialized);
}

/// The disks the storage drivers attached, in the order the root search
/// tries them.
#[cfg(target_arch = "x86_64")]
fn disks() -> alloc::vec::Vec<alloc::boxed::Box<dyn crate::block::BlockDevice>> {
    crate::block::devices()
}

/// The disks the storage drivers attached: VirtIO MMIO block devices, then
/// AHCI SATA disks, as the root search tries them.
#[cfg(target_arch = "aarch64")]
fn disks() -> alloc::vec::Vec<alloc::boxed::Box<dyn crate::block::BlockDevice>> {
    use crate::block::virtio::VirtioBlockWrapper;
    use crate::drivers::virtio::block_mmio::MAX_BLOCK_DEVICES;

    let mut disks: alloc::vec::Vec<alloc::boxed::Box<dyn crate::block::BlockDevice>> =
        alloc::vec::Vec::new();
    for index in 0..MAX_BLOCK_DEVICES {
        if let Some(disk) = VirtioBlockWrapper::new(index) {
            disks.push(alloc::boxed::Box::new(disk));
        }
    }
    for index in 0..crate::drivers::ahci::sata_device_count() {
        if let Some(disk) = crate::drivers::ahci::get_block_device_by_index(index) {
            disks.push(alloc::boxed::Box::new(disk));
        }
    }
    disks
}

/// Read block 0 of each attached disk through the block layer, once the
/// drivers can complete I/O, and report each read.
pub fn read_storage() {
    let disks = disks();
    let mut read = 0;
    for (index, disk) in disks.iter().enumerate() {
        let mut block = alloc::vec![0u8; disk.block_size()];
        match disk.read_block(0, &mut block) {
            Ok(()) => {
                read += 1;
                line(
                    "storage read",
                    format_args!(
                        "disk {} of {}, block 0, {} bytes",
                        index,
                        disks.len(),
                        block.len()
                    ),
                );
            }
            Err(error) => line(
                "storage read failed",
                format_args!("disk {} of {}: {}", index, disks.len(), error),
            ),
        }
    }
    if read == 0 {
        line(
            "storage not read",
            format_args!("{} disk(s) attached, none answered", disks.len()),
        );
        return;
    }
    #[cfg(target_arch = "aarch64")]
    screen(crate::graphics::boot_screen::Stage::StorageRead);
}
