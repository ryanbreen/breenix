//! The pieces the metal EFI applications share (docs/metal/README.md, "Delivery" and "Boot manager and persistent
//! install"): the UEFI declarations, COM1 progress lines, a boot disk attached from memory and started, files on a
//! FAT volume, and an HTTP client. Clean room: written only from the UEFI and PE/COFF specifications.
#![cfg_attr(not(test), no_std)]

pub mod bootdisk;
pub mod com1;
pub mod files;
pub mod firmware;
pub mod http;
pub mod sha256;
pub mod uefi;

pub mod slot_transaction;

pub mod slot_store;

pub mod live_flash;
pub mod breenfs_files;

#[cfg(test)]
#[path = "../tests/live_flash.rs"]
mod live_flash_tests;
