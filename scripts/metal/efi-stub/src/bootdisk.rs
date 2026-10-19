//! A boot disk in memory as a block device, and the bootloader started from its FAT volume
//! (docs/metal/README.md, "Delivery"). The delivery stub attaches the disk appended to it; the boot manager attaches a
//! kernel slot it read from the EFI system partition.
//!
//! The disk is registered through the firmware's RAM disk protocol when the firmware has one. Otherwise a read-only
//! Block I/O protocol and a RAM disk device path are installed for it. The firmware's partition and FAT drivers then
//! bind to it, and the pinned bootloader is started from the FAT volume by device path, so its own device is that
//! volume and it finds `kernel-x86_64`, `boot.json` and `ramdisk` there.
use core::ffi::c_void;
use core::mem::size_of;
use core::ptr::{self, null, null_mut};

use crate::say;
use crate::uefi::*;

pub const SECTOR: usize = 512;
pub const BOOTLOADER: &str = "\\EFI\\BOOT\\BOOTX64.EFI";

#[repr(C)]
struct RamDiskProtocol {
    register: unsafe extern "efiapi" fn(u64, u64, *const Guid, *const u8, *mut *mut u8) -> Status,
    _unregister: usize,
}

const BLOCK_IO_REVISION: u64 = 0x0001_0000;
/// A RAM disk media device path node: header, start and end addresses, disk type and instance.
const RAM_DISK_NODE: usize = 38;

/// The block device installed for a disk: the protocol first, so the protocol pointer the firmware passes back is this.
#[repr(C)]
struct MemoryDisk {
    block_io: BlockIo,
    media: BlockIoMedia,
    base: *const u8,
    path: [u8; RAM_DISK_NODE + END_NODE.len()],
}

/// Checks that `size` bytes at `base` look like a GPT boot disk.
///
/// # Safety
/// `base..base+size` must be readable memory.
pub unsafe fn looks_like_disk(base: *const u8, size: usize) -> bool {
    size >= 2 * SECTOR && size % SECTOR == 0 && core::slice::from_raw_parts(base.add(SECTOR), 8) == b"EFI PART"
}

/// The disk as a block device: through the firmware's RAM disk protocol when it has one, else an installed one.
/// Returns the disk's device path.
///
/// # Safety
/// `base..base+size` must stay valid, unchanged memory for as long as the firmware may read the disk.
pub unsafe fn attach(boot: &BootServices, base: *const u8, size: usize, what: &str) -> Result<*const u8, Status> {
    let mut protocol: *mut c_void = null_mut();
    let status = (boot.locate_protocol)(&RAM_DISK, null(), &mut protocol);
    if status == SUCCESS && !protocol.is_null() {
        let ram_disk = &*(protocol as *const RamDiskProtocol);
        let mut path: *mut u8 = null_mut();
        let status = (ram_disk.register)(base as u64, size as u64, &VIRTUAL_DISK, null(), &mut path);
        if status == SUCCESS && !path.is_null() {
            say!("registered the {} with the firmware's RAM disk protocol", what);
            return Ok(path);
        }
        say!("the firmware's RAM disk protocol refused the {} ({:#x}); using a block device of our own", what, status);
    } else {
        say!("the firmware has no RAM disk protocol ({:#x}); using a block device of our own", status);
    }
    install_block_device(boot, base, size, what)
}

unsafe fn install_block_device(boot: &BootServices, base: *const u8, size: usize, what: &str) -> Result<*const u8, Status> {
    let mut memory: *mut c_void = null_mut();
    let status = (boot.allocate_pool)(LOADER_DATA, size_of::<MemoryDisk>(), &mut memory);
    if status != SUCCESS || memory.is_null() {
        say!("could not allocate the {}'s block device ({:#x})", what, status);
        return Err(if status == SUCCESS { OUT_OF_RESOURCES } else { status });
    }
    let disk = memory as *mut MemoryDisk;
    let mut path = [0u8; RAM_DISK_NODE + END_NODE.len()];
    path[..4].copy_from_slice(&[node::MEDIA, node::RAM_DISK, RAM_DISK_NODE as u8, 0]);
    path[4..12].copy_from_slice(&(base as u64).to_le_bytes());
    path[12..20].copy_from_slice(&(base as u64 + size as u64 - 1).to_le_bytes());
    path[20..36].copy_from_slice(&VIRTUAL_DISK.bytes());
    path[RAM_DISK_NODE..].copy_from_slice(&END_NODE);
    ptr::write(
        disk,
        MemoryDisk {
            block_io: BlockIo {
                revision: BLOCK_IO_REVISION,
                media: null(),
                reset,
                read_blocks,
                write_blocks,
                flush_blocks,
            },
            media: BlockIoMedia {
                media_id: 1,
                removable_media: 0,
                media_present: 1,
                logical_partition: 0,
                read_only: 1,
                write_caching: 0,
                block_size: SECTOR as u32,
                io_align: 0,
                last_block: (size / SECTOR) as u64 - 1,
            },
            base,
            path,
        },
    );
    (*disk).block_io.media = ptr::addr_of!((*disk).media);
    let path = ptr::addr_of_mut!((*disk).path) as *mut u8;
    let mut handle: Handle = null_mut();
    let status = (boot.install_protocol_interface)(&mut handle, &DEVICE_PATH, NATIVE_INTERFACE, path as *mut c_void);
    if status != SUCCESS {
        say!("could not install the {}'s device path ({:#x})", what, status);
        return Err(status);
    }
    let status = (boot.install_protocol_interface)(&mut handle, &BLOCK_IO, NATIVE_INTERFACE, disk as *mut c_void);
    if status != SUCCESS {
        say!("could not install the {}'s block device ({:#x})", what, status);
        return Err(status);
    }
    say!("installed a read-only block device for the {}", what);
    Ok(path)
}

unsafe extern "efiapi" fn reset(_this: *mut BlockIo, _extended: u8) -> Status {
    SUCCESS
}

unsafe extern "efiapi" fn read_blocks(this: *mut BlockIo, media_id: u32, lba: u64, size: usize, buffer: *mut c_void) -> Status {
    let disk = &*(this as *const MemoryDisk);
    if media_id != disk.media.media_id {
        return MEDIA_CHANGED;
    }
    if size == 0 {
        return SUCCESS;
    }
    if buffer.is_null() {
        return INVALID_PARAMETER;
    }
    if size % disk.media.block_size as usize != 0 {
        return BAD_BUFFER_SIZE;
    }
    let blocks = disk.media.last_block + 1;
    let count = (size / disk.media.block_size as usize) as u64;
    if lba >= blocks || count > blocks - lba {
        return INVALID_PARAMETER;
    }
    ptr::copy_nonoverlapping(disk.base.add(lba as usize * disk.media.block_size as usize), buffer as *mut u8, size);
    SUCCESS
}

unsafe extern "efiapi" fn write_blocks(_this: *mut BlockIo, _media_id: u32, _lba: u64, _size: usize, _buffer: *const c_void) -> Status {
    WRITE_PROTECTED
}

unsafe extern "efiapi" fn flush_blocks(_this: *mut BlockIo) -> Status {
    SUCCESS
}

/// Connects the drivers to the disk at `disk` (its device path) and returns its FAT volume's device path.
///
/// # Safety
/// `disk` must be a device path the firmware has a Block I/O handle for.
pub unsafe fn connect(boot: &BootServices, disk: *const u8, what: &str) -> Result<*const u8, Status> {
    let mut remaining = disk;
    let mut controller: Handle = null_mut();
    let status = (boot.locate_device_path)(&BLOCK_IO, &mut remaining, &mut controller);
    if status != SUCCESS {
        say!("the {} has no block device ({:#x})", what, status);
        return Err(status);
    }
    let status = (boot.connect_controller)(controller, null(), null(), 1);
    say!("connected the {}'s drivers ({:#x})", what, status);
    find_volume(boot, disk, what)
}

/// The device path of the FAT volume the firmware found on the disk: the file system whose path extends the disk's.
unsafe fn find_volume(boot: &BootServices, disk: *const u8, what: &str) -> Result<*const u8, Status> {
    let prefix = path_length(disk).ok_or(INVALID_PARAMETER)?;
    let mut count = 0usize;
    let mut handles: *mut Handle = null_mut();
    let status = (boot.locate_handle_buffer)(BY_PROTOCOL, &SIMPLE_FILE_SYSTEM, null(), &mut count, &mut handles);
    if status != SUCCESS {
        say!("no file system appeared ({:#x})", status);
        return Err(status);
    }
    let mut found = None;
    for index in 0..count {
        let mut path: *mut c_void = null_mut();
        if (boot.handle_protocol)(*handles.add(index), &DEVICE_PATH, &mut path) != SUCCESS || path.is_null() {
            continue;
        }
        let path = path as *const u8;
        if let Some(length) = path_length(path) {
            if length > prefix && core::slice::from_raw_parts(path, prefix) == core::slice::from_raw_parts(disk, prefix) {
                found = Some(path);
                break;
            }
        }
    }
    (boot.free_pool)(handles as *mut c_void);
    match found {
        Some(path) => {
            say!("found the {}'s FAT volume ({} file systems in all)", what, count);
            Ok(path)
        }
        None => {
            say!("no FAT volume appeared on the {} ({} file systems in all)", what, count);
            Err(NOT_FOUND)
        }
    }
}

/// `volume`'s device path followed by a file path node naming `file`, in pool memory.
///
/// # Safety
/// `volume` must be a readable device path.
pub unsafe fn file_path(boot: &BootServices, volume: *const u8, file: &str) -> Result<*const u8, Status> {
    let prefix = path_length(volume).ok_or(INVALID_PARAMETER)?;
    let node = 4 + 2 * (file.len() + 1);
    let mut memory: *mut c_void = null_mut();
    let status = (boot.allocate_pool)(LOADER_DATA, prefix + node + END_NODE.len(), &mut memory);
    if status != SUCCESS || memory.is_null() {
        say!("could not allocate a file path ({:#x})", status);
        return Err(if status == SUCCESS { OUT_OF_RESOURCES } else { status });
    }
    let out = memory as *mut u8;
    ptr::copy_nonoverlapping(volume, out, prefix);
    let mut at = prefix;
    for byte in [node::MEDIA, node::FILE_PATH, node as u8, (node >> 8) as u8] {
        *out.add(at) = byte;
        at += 1;
    }
    for unit in file.bytes().map(u16::from).chain([0]) {
        for byte in unit.to_le_bytes() {
            *out.add(at) = byte;
            at += 1;
        }
    }
    for byte in END_NODE {
        *out.add(at) = byte;
        at += 1;
    }
    Ok(out)
}

/// Loads and starts the bootloader from `volume`. Returns only when it could not be started, or returned.
///
/// # Safety
/// `volume` must be a FAT volume's device path; `image` this application's handle.
pub unsafe fn start_bootloader(boot: &BootServices, image: Handle, volume: *const u8, what: &str) -> Status {
    let file = match file_path(boot, volume, BOOTLOADER) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let mut child: Handle = null_mut();
    let status = (boot.load_image)(0, image, file, null(), 0, &mut child);
    if status != SUCCESS {
        say!("could not load {} from the {} ({:#x})", BOOTLOADER, what, status);
        return status;
    }
    say!("starting the bootloader");
    let status = (boot.start_image)(child, null_mut(), null_mut());
    say!("the bootloader returned ({:#x})", status);
    if status == SUCCESS { LOAD_ERROR } else { status }
}
