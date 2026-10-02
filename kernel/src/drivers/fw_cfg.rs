//! QEMU fw_cfg device driver
//!
//! Reads configuration data passed from the host via QEMU's `-fw_cfg` option.
//! On the ARM64 virt machine, fw_cfg is at MMIO base 0x09020000; on x86-64 it is
//! the I/O ports 0x510 (selector) and 0x511 (data), present only when the
//! signature reads "QEMU".
//!
//! Usage from run.sh:
//!   -fw_cfg name=opt/breenix/resolution,string=1728x1080

#[cfg(target_arch = "aarch64")]
use core::ptr::{read_volatile, write_volatile};

/// MMIO base address for fw_cfg on QEMU ARM64 virt machine
#[cfg(target_arch = "aarch64")]
const FW_CFG_BASE: u64 = 0x0902_0000;

/// fw_cfg register offsets
#[cfg(target_arch = "aarch64")]
const FW_CFG_DATA: u64 = 0x000;
#[cfg(target_arch = "aarch64")]
const FW_CFG_SELECTOR: u64 = 0x008;

/// fw_cfg I/O ports on x86-64
#[cfg(target_arch = "x86_64")]
const FW_CFG_PORT_SELECTOR: u16 = 0x510;
#[cfg(target_arch = "x86_64")]
const FW_CFG_PORT_DATA: u16 = 0x511;

/// Well-known selectors
#[cfg(target_arch = "x86_64")]
const FW_CFG_SIGNATURE: u16 = 0x0000;
const FW_CFG_FILE_DIR: u16 = 0x0019;

/// Convert physical address to virtual (kernel high-half mapping)
#[cfg(target_arch = "aarch64")]
#[inline]
fn fw_cfg_virt(offset: u64) -> *mut u8 {
    let phys = FW_CFG_BASE + offset;
    (phys + crate::memory::physical_memory_offset().as_u64()) as *mut u8
}

/// Write the selector register (16-bit big-endian)
#[cfg(target_arch = "aarch64")]
#[inline]
fn select(sel: u16) {
    unsafe {
        let addr = fw_cfg_virt(FW_CFG_SELECTOR) as *mut u16;
        write_volatile(addr, sel.to_be());
    }
}

/// Write the selector port (16-bit, host order)
#[cfg(target_arch = "x86_64")]
#[inline]
fn select(sel: u16) {
    use x86_64::instructions::port::Port;
    unsafe { Port::<u16>::new(FW_CFG_PORT_SELECTOR).write(sel) }
}

/// Read one byte from the data register
#[cfg(target_arch = "aarch64")]
#[inline]
fn read_byte() -> u8 {
    unsafe { read_volatile(fw_cfg_virt(FW_CFG_DATA)) }
}

/// Read one byte from the data port
#[cfg(target_arch = "x86_64")]
#[inline]
fn read_byte() -> u8 {
    use x86_64::instructions::port::Port;
    unsafe { Port::<u8>::new(FW_CFG_PORT_DATA).read() }
}

/// Whether the fw_cfg device is there to read. ARM64 callers check
/// `platform_config::is_qemu()` first; on x86-64 the signature says.
#[cfg(target_arch = "aarch64")]
fn present() -> bool {
    true
}

#[cfg(target_arch = "x86_64")]
fn present() -> bool {
    select(FW_CFG_SIGNATURE);
    let mut signature = [0u8; 4];
    read_bytes(&mut signature);
    &signature == b"QEMU"
}

/// Read n bytes sequentially from the data register
fn read_bytes(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        *b = read_byte();
    }
}

/// Read a big-endian u32 from data register (4 sequential byte reads)
fn read_be32() -> u32 {
    let mut buf = [0u8; 4];
    read_bytes(&mut buf);
    u32::from_be_bytes(buf)
}

/// Read a big-endian u16 from data register
fn read_be16() -> u16 {
    let mut buf = [0u8; 2];
    read_bytes(&mut buf);
    u16::from_be_bytes(buf)
}

/// Look up a named file in fw_cfg and read its contents into buf (as much as
/// fits). Returns the file's full size, or 0 if it is not found.
pub fn read_file(name: &str, buf: &mut [u8]) -> usize {
    if !present() {
        return 0;
    }
    // Select the file directory
    select(FW_CFG_FILE_DIR);
    // Read file count (BE u32)
    let count = read_be32();

    // Each directory entry is 64 bytes: { be32 size, be16 select, u16 reserved, char[56] name }
    for _ in 0..count {
        let size = read_be32();
        let sel = read_be16();
        let _reserved = read_be16();

        let mut entry_name = [0u8; 56];
        read_bytes(&mut entry_name);

        // Compare name (null-terminated in entry)
        let entry_len = entry_name.iter().position(|&b| b == 0).unwrap_or(56);
        let entry_str = core::str::from_utf8(&entry_name[..entry_len]).unwrap_or("");

        if entry_str == name {
            // Found it — select this file and read its data
            let to_read = core::cmp::min(size as usize, buf.len());
            select(sel);
            read_bytes(&mut buf[..to_read]);
            return size as usize;
        }
    }

    0 // Not found
}

/// Read a string value from fw_cfg. Returns None if not found. A value too long
/// to read whole comes back cut short and ending in "...", so no caller mistakes
/// it for the value that was given.
pub fn read_string(name: &str) -> Option<alloc::string::String> {
    let mut buf = [0u8; 256];
    let size = read_file(name, &mut buf);
    if size == 0 {
        return None;
    }
    let n = size.min(buf.len());
    // Trim any trailing null/whitespace
    let s = core::str::from_utf8(&buf[..n]).ok()?;
    let mut value = alloc::string::String::from(s.trim_end_matches('\0').trim());
    if size > buf.len() {
        value.push_str("...");
    }
    Some(value)
}
