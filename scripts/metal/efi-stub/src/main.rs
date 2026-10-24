//! metal-efi-stub: the x86-metal boot payload (docs/metal/README.md, "Delivery").
//!
//! The server's firmware HTTP boot starts only a bare EFI application, while the pinned bootloader loads its kernel,
//! `boot.json` and optional `ramdisk` from the FAT file system on the device it was itself loaded from. The tools
//! append a GPT boot disk (one FAT EFI system partition holding `efi/boot/bootx64.efi` and those files) to this
//! application as a section and record where it is in [`PAYLOAD`] (`stub_efi` in `scripts/metal/metal_image.py`).
//! The stub makes that disk a block device, lets the firmware's partition and FAT drivers bind to it, and starts
//! the bootloader from the FAT volume by device path, so the bootloader's own device is that volume
//! (`metal_efi::bootdisk`).
//!
//! Clean room: written only from the UEFI specification (system table, boot services, device paths, and the Block
//! I/O, Simple File System and RAM disk protocols) and the PE/COFF specification. Progress lines, each starting
//! `metal-stub:`, go to the COM1 UART, which the firmware's console redirection and the remote console share.
#![no_std]
#![no_main]

use core::ptr;

use metal_efi::bootdisk;
use metal_efi::com1::{self, Vendor};
use metal_efi::say;
use metal_efi::uefi::*;

mod persist;
mod suite_disk;

/// Where the appended disk is: the tools find this record by its magic and fill in the disk's distance from the
/// record and its length. Read only through volatile loads, because its contents are not the initializer's.
#[repr(C)]
struct Payload {
    magic: [u8; 32],
    offset: i64,
    size: u64,
}

#[used]
static mut PAYLOAD: Payload = Payload { magic: *b"BREENIX-METAL-STUB-PAYLOAD-V1\0\0\0", offset: 0, size: 0 };

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("panic: {}", info.message());
    com1::halt()
}

#[no_mangle]
extern "efiapi" fn efi_main(image: Handle, system: *const SystemTable) -> Status {
    com1::set_prefix("metal-stub: ");
    // SAFETY: the firmware passes this image's handle and its system table, valid while boot services run.
    let status = unsafe { run(image, system) };
    say!("FAILED: status {:#x}; halting", status);
    com1::halt()
}

/// Attach the disk, find its FAT volume and start the bootloader from it. Returns only on failure.
unsafe fn run(image: Handle, system: *const SystemTable) -> Status {
    metal_efi::firmware::publish(system);
    let boot = &*(*system).boot_services;
    let record = ptr::addr_of!(PAYLOAD);
    let offset = ptr::read_volatile(ptr::addr_of!((*record).offset));
    let size = ptr::read_volatile(ptr::addr_of!((*record).size)) as usize;
    say!(
        "started: firmware {} revision {:#x}, boot disk {} bytes",
        Vendor((*system).firmware_vendor),
        (*system).firmware_revision,
        size
    );
    let base = (record as *const u8).offset(offset as isize);
    if offset == 0 || size < 2 * bootdisk::SECTOR || size % bootdisk::SECTOR != 0 {
        say!("no boot disk is appended to this stub");
        return LOAD_ERROR;
    }
    if !bootdisk::looks_like_disk(base, size) {
        say!("the appended boot disk has no GPT header");
        return LOAD_ERROR;
    }
    let disk = match bootdisk::attach(boot, base, size, "boot disk") {
        Ok(path) => path,
        Err(status) => return status,
    };
    let volume = match bootdisk::connect(boot, disk, "boot disk") {
        Ok(path) => path,
        Err(status) => return status,
    };
    // A persistent install's request (README, "Install and upgrade") comes before the test root it stages.
    if let Err(status) = persist::run(boot, &*(*system).runtime_services, volume, image) {
        return status;
    }
    if let Err(status) = suite_disk::run(boot, volume, image) {
        return status;
    }
    bootdisk::start_bootloader(boot, image, volume, "boot disk")
}
