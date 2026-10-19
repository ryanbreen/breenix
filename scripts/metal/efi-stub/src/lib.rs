//! The pieces the metal EFI applications share (docs/metal/README.md, "Delivery" and "Boot manager and persistent
//! install"): the UEFI declarations, COM1 progress lines, a boot disk attached from memory and started, files on a
//! FAT volume, and an HTTP client. Clean room: written only from the UEFI and PE/COFF specifications.
#![no_std]

pub mod bootdisk;
pub mod com1;
pub mod files;
pub mod http;
pub mod uefi;
