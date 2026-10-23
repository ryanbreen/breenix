//! Carry the UEFI SMBIOS entry point to the kernel through an ACPI UEFI data
//! table (UEFI 2.10 section 4.6, section 20.1 and Appendix O). The kernel's
//! bootloader handoff exposes RSDP but not the UEFI configuration array.
use core::ffi::c_void;
use core::ptr::{null, null_mut};
use crate::uefi::*;

// Breenix SMBIOS handoff identifier, in EFI mixed-endian byte order.
pub const IDENTIFIER: [u8; 16] = [0x0b,0x76,0xb1,0x32,0x04,0x68,0x9c,0x4f,0xab,0x21,0x67,0xf5,0x8f,0x56,0x42,0xe1];
const SMBIOS: Guid = Guid(0xeb9d2d31,0x2d88,0x11d3,[0x9a,0x16,0,0x90,0x27,0x3f,0xc1,0x4d]);
const SMBIOS3: Guid = Guid(0xf2fd1544,0x9794,0x4a2c,[0x99,0x2e,0xe5,0xbb,0xcf,0x20,0xe3,0x94]);
const ACPI_TABLE: Guid = Guid(0xffe06bdd,0x6107,0x46a6,[0x7b,0xb2,0x5a,0x9c,0x7e,0xc5,0x27,0x5c]);
#[repr(C)]
struct Configuration { guid: Guid, address: *const c_void }
#[repr(C)]
struct Tables {
    install: unsafe extern "efiapi" fn(*mut Tables, *const c_void, usize, *mut usize) -> Status,
    uninstall: usize,
}
/// # Safety
/// `system` is the firmware's live system table, before ExitBootServices.
pub unsafe fn publish(system: *const SystemTable) {
    let system = &*system;
    if system.number_of_table_entries > 1024 || system.configuration_table.is_null() { return; }
    let entries = core::slice::from_raw_parts(system.configuration_table.cast::<Configuration>(), system.number_of_table_entries);
    let entry = entries.iter().find(|e| e.guid == SMBIOS3).or_else(|| entries.iter().find(|e| e.guid == SMBIOS));
    let Some(entry) = entry.filter(|e| !e.address.is_null()) else { return; };
    let mut protocol = null_mut();
    if ((*system.boot_services).locate_protocol)(&ACPI_TABLE, null(), &mut protocol) != SUCCESS || protocol.is_null() { return; }
    let mut table = [0u8; 62];
    table[..4].copy_from_slice(b"UEFI");
    table[4..8].copy_from_slice(&62u32.to_le_bytes());
    table[8] = 1;
    table[10..16].copy_from_slice(b"BREENX");
    table[16..24].copy_from_slice(b"SMBIOS  ");
    table[24..28].copy_from_slice(&1u32.to_le_bytes());
    table[28..32].copy_from_slice(b"BRNX");
    table[32..36].copy_from_slice(&1u32.to_le_bytes());
    table[36..52].copy_from_slice(&IDENTIFIER);
    table[52..54].copy_from_slice(&54u16.to_le_bytes());
    table[54..62].copy_from_slice(&(entry.address as u64).to_le_bytes());
    table[9] = 0u8.wrapping_sub(table.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
    let protocol = protocol.cast::<Tables>();
    let mut key = 0;
    let status = ((*protocol).install)(protocol, table.as_ptr().cast(), table.len(), &mut key);
    if status == SUCCESS { crate::say!("SMBIOS handoff published"); }
}
