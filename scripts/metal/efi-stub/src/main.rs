//! metal-efi-stub: the x86-metal boot payload (docs/metal/README.md, "Delivery").
//!
//! The server's firmware HTTP boot starts only a bare EFI application, while the pinned bootloader loads its kernel,
//! `boot.json` and optional `ramdisk` from the FAT file system on the device it was itself loaded from. The tools
//! append a GPT boot disk (one FAT EFI system partition holding `efi/boot/bootx64.efi` and those files) to this
//! application as a section and record where it is in [`PAYLOAD`] (`stub_efi` in `scripts/metal/metal_image.py`).
//! The stub makes that disk a block device, lets the firmware's partition and FAT drivers bind to it, and starts
//! the bootloader from the FAT volume by device path, so the bootloader's own device is that volume.
//!
//! The disk is registered through the firmware's RAM disk protocol when the firmware has one. Otherwise the stub
//! installs a read-only Block I/O protocol and a RAM disk device path for it itself.
//!
//! Clean room: written only from the UEFI specification (system table, boot services, device paths, and the Block
//! I/O, Simple File System and RAM disk protocols) and the PE/COFF specification. Progress lines, each starting
//! `metal-stub:`, go to the COM1 UART, which the firmware's console redirection and the remote console share.
#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::c_void;
use core::fmt::{self, Write};
use core::mem::size_of;
use core::ptr::{self, null, null_mut};

type Handle = *mut c_void;
type Status = usize;

const ERROR: usize = 1 << (usize::BITS - 1);
const SUCCESS: Status = 0;
const LOAD_ERROR: Status = ERROR | 1;
const INVALID_PARAMETER: Status = ERROR | 2;
const BAD_BUFFER_SIZE: Status = ERROR | 4;
const WRITE_PROTECTED: Status = ERROR | 8;
const OUT_OF_RESOURCES: Status = ERROR | 9;
const MEDIA_CHANGED: Status = ERROR | 13;
const NOT_FOUND: Status = ERROR | 14;

/// `EfiLoaderData`, the memory type an application allocates its own data as.
const LOADER_DATA: u32 = 2;
/// `EFI_NATIVE_INTERFACE`.
const NATIVE_INTERFACE: u32 = 0;
/// `ByProtocol`, the `LocateHandleBuffer` search type.
const BY_PROTOCOL: u32 = 2;

const SECTOR: usize = 512;
const BOOTLOADER: &str = "\\EFI\\BOOT\\BOOTX64.EFI";

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid(u32, u16, u16, [u8; 8]);

const DEVICE_PATH: Guid = Guid(0x0957_6e91, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const BLOCK_IO: Guid = Guid(0x964e_5b21, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const SIMPLE_FILE_SYSTEM: Guid = Guid(0x964e_5b22, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const RAM_DISK: Guid = Guid(0xab38_a0df, 0x6873, 0x44a9, [0x87, 0xe6, 0xd4, 0xeb, 0x56, 0x14, 0x84, 0x49]);
/// `EFI_VIRTUAL_DISK_GUID`: a RAM disk holding a raw disk image.
const VIRTUAL_DISK: Guid = Guid(0x77ab_535a, 0x45fc, 0x624b, [0x55, 0x60, 0xf7, 0xb2, 0x81, 0xd1, 0xf9, 0x6e]);

#[repr(C)]
struct TableHeader {
    _signature: u64,
    _revision: u32,
    _header_size: u32,
    _crc32: u32,
    _reserved: u32,
}

#[repr(C)]
struct SystemTable {
    _header: TableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    /// ConsoleInHandle through RuntimeServices: the consoles and runtime services, which the stub does not use.
    _consoles_and_runtime: [usize; 7],
    boot_services: *const BootServices,
    _configuration_tables: [usize; 2],
}

/// The boot services table in the specification's order; the services the stub does not call are placeholders.
#[repr(C)]
struct BootServices {
    _header: TableHeader,
    /// RaiseTPL, RestoreTPL, AllocatePages, FreePages, GetMemoryMap.
    _tpl_and_pages: [usize; 5],
    allocate_pool: unsafe extern "efiapi" fn(u32, usize, *mut *mut c_void) -> Status,
    free_pool: unsafe extern "efiapi" fn(*mut c_void) -> Status,
    /// CreateEvent, SetTimer, WaitForEvent, SignalEvent, CloseEvent, CheckEvent.
    _events: [usize; 6],
    install_protocol_interface: unsafe extern "efiapi" fn(*mut Handle, *const Guid, u32, *mut c_void) -> Status,
    /// ReinstallProtocolInterface, UninstallProtocolInterface.
    _reinstall_uninstall: [usize; 2],
    handle_protocol: unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    /// Reserved, RegisterProtocolNotify, LocateHandle.
    _reserved_notify_locate: [usize; 3],
    locate_device_path: unsafe extern "efiapi" fn(*const Guid, *mut *const u8, *mut Handle) -> Status,
    _install_configuration_table: usize,
    load_image: unsafe extern "efiapi" fn(u8, Handle, *const u8, *const c_void, usize, *mut Handle) -> Status,
    start_image: unsafe extern "efiapi" fn(Handle, *mut usize, *mut *mut u16) -> Status,
    /// Exit, UnloadImage, ExitBootServices, GetNextMonotonicCount, Stall, SetWatchdogTimer.
    _exit_to_watchdog: [usize; 6],
    connect_controller: unsafe extern "efiapi" fn(Handle, *const Handle, *const u8, u8) -> Status,
    /// DisconnectController, OpenProtocol, CloseProtocol, OpenProtocolInformation, ProtocolsPerHandle.
    _disconnect_to_protocols_per_handle: [usize; 5],
    locate_handle_buffer: unsafe extern "efiapi" fn(u32, *const Guid, *const c_void, *mut usize, *mut *mut Handle) -> Status,
    locate_protocol: unsafe extern "efiapi" fn(*const Guid, *const c_void, *mut *mut c_void) -> Status,
}

#[repr(C)]
struct RamDiskProtocol {
    register: unsafe extern "efiapi" fn(u64, u64, *const Guid, *const u8, *mut *mut u8) -> Status,
    _unregister: usize,
}

/// `EFI_BLOCK_IO_MEDIA`, revision 1. The firmware reads it through the protocol; the stub's own Block I/O reads
/// only the fields it checks requests against.
#[repr(C)]
struct BlockIoMedia {
    media_id: u32,
    removable_media: u8,
    media_present: u8,
    logical_partition: u8,
    read_only: u8,
    write_caching: u8,
    block_size: u32,
    io_align: u32,
    last_block: u64,
}

#[repr(C)]
struct BlockIo {
    revision: u64,
    media: *const BlockIoMedia,
    reset: unsafe extern "efiapi" fn(*mut BlockIo, u8) -> Status,
    read_blocks: unsafe extern "efiapi" fn(*mut BlockIo, u32, u64, usize, *mut c_void) -> Status,
    write_blocks: unsafe extern "efiapi" fn(*mut BlockIo, u32, u64, usize, *const c_void) -> Status,
    flush_blocks: unsafe extern "efiapi" fn(*mut BlockIo) -> Status,
}

const BLOCK_IO_REVISION: u64 = 0x0001_0000;
/// A RAM disk media device path node: header, start and end addresses, disk type and instance.
const RAM_DISK_NODE: usize = 38;
const END_NODE: [u8; 4] = [0x7f, 0xff, 4, 0];

/// The stub's own block device: the protocol first, so the protocol pointer the firmware passes back is this.
#[repr(C)]
struct StubDisk {
    block_io: BlockIo,
    media: BlockIoMedia,
    base: *const u8,
    path: [u8; RAM_DISK_NODE + END_NODE.len()],
}

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

struct Com1;

impl Write for Com1 {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                put(b'\r');
            }
            put(byte);
        }
        Ok(())
    }
}

/// One byte to COM1 once its transmit holding register is empty, or after a bounded wait if no UART answers.
fn put(byte: u8) {
    for _ in 0..100_000 {
        let status: u8;
        // SAFETY: COM1's line status register; reading it has no side effect on the transmitter.
        unsafe { asm!("in al, dx", out("al") status, in("dx") 0x3fdu16, options(nomem, nostack)) };
        if status & 0x20 != 0 {
            break;
        }
    }
    // SAFETY: COM1's transmit holding register.
    unsafe { asm!("out dx, al", in("al") byte, in("dx") 0x3f8u16, options(nomem, nostack)) };
}

macro_rules! say {
    ($($arg:tt)*) => {
        let _ = write!(Com1, "metal-stub: {}\n", format_args!($($arg)*));
    };
}

mod suite_disk;

/// The firmware's vendor string, printable ASCII only.
struct Vendor(*const u16);

impl fmt::Display for Vendor {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result {
        if self.0.is_null() {
            return out.write_str("(none)");
        }
        for index in 0..64 {
            // SAFETY: the system table's NUL-terminated vendor string, read up to its terminator or 64 units.
            let unit = unsafe { *self.0.add(index) };
            if unit == 0 {
                break;
            }
            let shown = if (0x20..0x7f).contains(&unit) { unit as u8 as char } else { '?' };
            out.write_char(shown)?;
        }
        Ok(())
    }
}

fn halt() -> ! {
    loop {
        // SAFETY: masks interrupts and halts the processor for good.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("panic: {}", info.message());
    halt()
}

#[no_mangle]
extern "efiapi" fn efi_main(image: Handle, system: *const SystemTable) -> Status {
    // SAFETY: the firmware passes this image's handle and its system table, valid while boot services run.
    let status = unsafe { run(image, system) };
    say!("FAILED: status {:#x}; halting", status);
    halt()
}

/// Attach the disk, find its FAT volume and start the bootloader from it. Returns only on failure.
unsafe fn run(image: Handle, system: *const SystemTable) -> Status {
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
    if offset == 0 || size < 2 * SECTOR || size % SECTOR != 0 {
        say!("no boot disk is appended to this stub");
        return LOAD_ERROR;
    }
    if core::slice::from_raw_parts(base.add(SECTOR), 8) != b"EFI PART" {
        say!("the appended boot disk has no GPT header");
        return LOAD_ERROR;
    }
    let disk = match attach(boot, base, size) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let mut remaining = disk;
    let mut controller: Handle = null_mut();
    let status = (boot.locate_device_path)(&BLOCK_IO, &mut remaining, &mut controller);
    if status != SUCCESS {
        say!("the boot disk has no block device ({:#x})", status);
        return status;
    }
    let status = (boot.connect_controller)(controller, null(), null(), 1);
    say!("connected the boot disk's drivers ({:#x})", status);
    let volume = match find_volume(boot, disk) {
        Ok(path) => path,
        Err(status) => return status,
    };
    if let Err(status) = suite_disk::run(boot, volume) {
        return status;
    }
    let file = match file_path(boot, volume, BOOTLOADER) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let mut child: Handle = null_mut();
    let status = (boot.load_image)(0, image, file, null(), 0, &mut child);
    if status != SUCCESS {
        say!("could not load {} from the boot disk ({:#x})", BOOTLOADER, status);
        return status;
    }
    say!("starting the bootloader");
    let status = (boot.start_image)(child, null_mut(), null_mut());
    say!("the bootloader returned ({:#x})", status);
    if status == SUCCESS { LOAD_ERROR } else { status }
}

/// The disk as a block device: through the firmware's RAM disk protocol when it has one, else the stub's own.
/// Returns the disk's device path.
unsafe fn attach(boot: &BootServices, base: *const u8, size: usize) -> Result<*const u8, Status> {
    let mut protocol: *mut c_void = null_mut();
    let status = (boot.locate_protocol)(&RAM_DISK, null(), &mut protocol);
    if status == SUCCESS && !protocol.is_null() {
        let ram_disk = &*(protocol as *const RamDiskProtocol);
        let mut path: *mut u8 = null_mut();
        let status = (ram_disk.register)(base as u64, size as u64, &VIRTUAL_DISK, null(), &mut path);
        if status == SUCCESS && !path.is_null() {
            say!("registered the boot disk with the firmware's RAM disk protocol");
            return Ok(path);
        }
        say!("the firmware's RAM disk protocol refused the boot disk ({:#x}); using the stub's block device", status);
    } else {
        say!("the firmware has no RAM disk protocol ({:#x}); using the stub's block device", status);
    }
    install_block_device(boot, base, size)
}

unsafe fn install_block_device(boot: &BootServices, base: *const u8, size: usize) -> Result<*const u8, Status> {
    let mut memory: *mut c_void = null_mut();
    let status = (boot.allocate_pool)(LOADER_DATA, size_of::<StubDisk>(), &mut memory);
    if status != SUCCESS || memory.is_null() {
        say!("could not allocate the stub's block device ({:#x})", status);
        return Err(if status == SUCCESS { OUT_OF_RESOURCES } else { status });
    }
    let disk = memory as *mut StubDisk;
    let mut path = [0u8; RAM_DISK_NODE + END_NODE.len()];
    path[..4].copy_from_slice(&[4, 9, RAM_DISK_NODE as u8, 0]);
    path[4..12].copy_from_slice(&(base as u64).to_le_bytes());
    path[12..20].copy_from_slice(&(base as u64 + size as u64 - 1).to_le_bytes());
    path[20..24].copy_from_slice(&VIRTUAL_DISK.0.to_le_bytes());
    path[24..26].copy_from_slice(&VIRTUAL_DISK.1.to_le_bytes());
    path[26..28].copy_from_slice(&VIRTUAL_DISK.2.to_le_bytes());
    path[28..36].copy_from_slice(&VIRTUAL_DISK.3);
    path[RAM_DISK_NODE..].copy_from_slice(&END_NODE);
    ptr::write(
        disk,
        StubDisk {
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
        say!("could not install the stub disk's device path ({:#x})", status);
        return Err(status);
    }
    let status = (boot.install_protocol_interface)(&mut handle, &BLOCK_IO, NATIVE_INTERFACE, disk as *mut c_void);
    if status != SUCCESS {
        say!("could not install the stub disk's block device ({:#x})", status);
        return Err(status);
    }
    say!("installed the stub's read-only block device for the boot disk");
    Ok(path)
}

unsafe extern "efiapi" fn reset(_this: *mut BlockIo, _extended: u8) -> Status {
    SUCCESS
}

unsafe extern "efiapi" fn read_blocks(this: *mut BlockIo, media_id: u32, lba: u64, size: usize, buffer: *mut c_void) -> Status {
    let disk = &*(this as *const StubDisk);
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

/// Bytes of a device path before its end node; no length for a node shorter than its header or a path longer
/// than 64 nodes.
unsafe fn path_length(path: *const u8) -> Option<usize> {
    let mut at = 0;
    for _ in 0..64 {
        let node = core::slice::from_raw_parts(path.add(at), 4);
        if node[0] == END_NODE[0] && node[1] == END_NODE[1] {
            return Some(at);
        }
        let length = u16::from_le_bytes([node[2], node[3]]) as usize;
        if length < 4 {
            return None;
        }
        at += length;
    }
    None
}

/// The device path of the FAT volume the firmware found on the disk: the file system whose path extends the disk's.
unsafe fn find_volume(boot: &BootServices, disk: *const u8) -> Result<*const u8, Status> {
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
            say!("found the boot disk's FAT volume ({} file systems in all)", count);
            Ok(path)
        }
        None => {
            say!("no FAT volume appeared on the boot disk ({} file systems in all)", count);
            Err(NOT_FOUND)
        }
    }
}

/// `volume`'s device path followed by a file path node naming `file`, in pool memory.
unsafe fn file_path(boot: &BootServices, volume: *const u8, file: &str) -> Result<*const u8, Status> {
    let prefix = path_length(volume).ok_or(INVALID_PARAMETER)?;
    let node = 4 + 2 * (file.len() + 1);
    let mut memory: *mut c_void = null_mut();
    let status = (boot.allocate_pool)(LOADER_DATA, prefix + node + END_NODE.len(), &mut memory);
    if status != SUCCESS || memory.is_null() {
        say!("could not allocate the bootloader's file path ({:#x})", status);
        return Err(if status == SUCCESS { OUT_OF_RESOURCES } else { status });
    }
    let out = memory as *mut u8;
    ptr::copy_nonoverlapping(volume, out, prefix);
    let mut at = prefix;
    for byte in [4u8, 4, node as u8, (node >> 8) as u8] {
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
