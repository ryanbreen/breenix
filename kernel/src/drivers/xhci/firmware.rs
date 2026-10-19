//! Firmware interrupt-controller descriptions, ACPI 6.4 sections 5.2.5,
//! 5.2.7-5.2.8 and 5.2.12.16. Tables are data, not driver implementations.
use core::sync::atomic::{AtomicU64, Ordering};

static RSDP: AtomicU64 = AtomicU64::new(0);

pub fn set_rsdp(address: u64) {
    RSDP.store(address, Ordering::Release);
}

#[derive(Clone, Copy)]
pub(super) struct MsiFrame {
    pub base: u64,
    pub range: Option<(u32, u32)>,
}

fn word(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}
fn dword(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}
fn qword(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}
fn checksum(data: &[u8]) -> bool {
    data.iter().fold(0u8, |sum, value| sum.wrapping_add(*value)) == 0
}

/// The end of the 48-bit physical address space the HHDM maps.
const PHYSICAL_END: u64 = 0x0001_0000_0000_0000;

// The loader supplies the RSDP; the HHDM retains the firmware's ACPI RAM.
// Lengths and checksums are checked before following entries in these tables.
unsafe fn bytes(address: u64, length: usize) -> Option<&'static [u8]> {
    use crate::arch_impl::aarch64::constants::HHDM_BASE;
    if address == 0 || address.checked_add(length as u64)? >= PHYSICAL_END {
        return None;
    }
    Some(core::slice::from_raw_parts(
        (HHDM_BASE + address) as *const u8,
        length,
    ))
}
unsafe fn table(address: u64) -> Option<&'static [u8]> {
    let header = bytes(address, 36)?;
    let length = dword(header, 4)? as usize;
    if !(36..=1024 * 1024).contains(&length) {
        return None;
    }
    let table = bytes(address, length)?;
    checksum(table).then_some(table)
}

fn frame(madt: &[u8]) -> Option<MsiFrame> {
    if madt.get(..4)? != b"APIC" || madt.len() < 44 {
        return None;
    }
    let mut at = 44;
    while at + 2 <= madt.len() {
        let length = madt[at + 1] as usize;
        if length < 2 || at + length > madt.len() {
            return None;
        }
        let entry = &madt[at..at + length];
        if entry[0] == 13 && length >= 24 {
            let base = qword(entry, 8)?;
            let flags = dword(entry, 16)?;
            let range = (flags & 1 != 0)
                .then(|| Some((word(entry, 22)? as u32, word(entry, 20)? as u32)))
                .flatten();
            return Some(MsiFrame { base, range });
        }
        at += length;
    }
    None
}

pub(super) fn msi_frame() -> Option<MsiFrame> {
    unsafe {
        let address = RSDP.load(Ordering::Acquire);
        let rsdp = bytes(address, 20)?;
        if &rsdp[..8] != b"RSD PTR " || !checksum(rsdp) {
            return None;
        }
        let (root, stride) = if rsdp[15] >= 2 {
            let extended = bytes(address, 36)?;
            let length = dword(extended, 20)? as usize;
            if !(36..=4096).contains(&length) || !checksum(bytes(address, length)?) {
                return None;
            }
            (qword(extended, 24)?, 8)
        } else {
            (dword(rsdp, 16)? as u64, 4)
        };
        let root = table(root)?;
        let signature: &[u8] = if stride == 8 { b"XSDT" } else { b"RSDT" };
        if &root[..4] != signature {
            return None;
        }
        for entry in root[36..].chunks_exact(stride) {
            let address = if stride == 8 {
                qword(entry, 0)?
            } else {
                dword(entry, 0)? as u64
            };
            let Some(child) = table(address) else {
                continue;
            };
            if &child[..4] == b"APIC" {
                return frame(child);
            }
        }
        None
    }
}
