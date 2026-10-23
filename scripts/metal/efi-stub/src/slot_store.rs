//! The recovered ESP slot transaction's file store, shared by manager and installer.
use crate::files::Root;
use crate::uefi::*;

pub struct SlotStore<'a>(pub &'a Root);

fn path<'a>(name: &str, out: &'a mut [u8; 96]) -> &'a str {
    let prefix = b"\\EFI\\BREENIX\\";
    out[..prefix.len()].copy_from_slice(prefix);
    out[prefix.len()..prefix.len() + name.len()].copy_from_slice(name.as_bytes());
    core::str::from_utf8(&out[..prefix.len() + name.len()]).unwrap_or("")
}

impl crate::slot_transaction::Store for SlotStore<'_> {
    type Error = Status;
    fn exists(&self, name: &str) -> bool {
        // Uncertainty must enter the read/rename path and propagate its error,
        // rather than turn a journal or retained slot into apparent absence.
        unsafe { self.0.size(path(name, &mut [0; 96])) != Err(NOT_FOUND) }
    }
    fn read(&self, name: &str, bytes: &mut [u8]) -> Result<usize, Status> {
        unsafe { self.0.read(path(name, &mut [0; 96]), bytes) }
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), Status> {
        unsafe { self.0.write(path(name, &mut [0; 96]), bytes) }
    }
    fn delete(&self, name: &str) -> Result<(), Status> {
        unsafe { self.0.delete(path(name, &mut [0; 96])) }
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), Status> {
        unsafe { self.0.rename(path(from, &mut [0; 96]), to) }
    }
    fn invalid(&self) -> Status { VOLUME_CORRUPTED }
}
