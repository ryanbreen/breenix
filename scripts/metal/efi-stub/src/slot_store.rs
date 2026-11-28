//! The recovered ESP slot transaction's file store, shared by manager and installer.
use crate::files::{self, Root};
use crate::live_flash::MAX_IMAGE;
use crate::slot_transaction::{RecordStore, RECORD_PATHS, RECORD_SIZE};
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
        unsafe { self.0.read_bounded(path(name, &mut [0; 96]), bytes) }
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
    fn invalid(&self) -> Status {
        VOLUME_CORRUPTED
    }
}

/// The boot services and device barrier for the open EFI system partition.
pub struct Esp<'a> {
    files: SlotStore<'a>,
    boot: &'a BootServices,
    device: Handle,
}

impl<'a> Esp<'a> {
    /// The handle and root must name the same firmware volume.
    pub unsafe fn new(root: &'a Root, boot: &'a BootServices, device: Handle) -> Self {
        Self {
            files: SlotStore(root),
            boot,
            device,
        }
    }
}

impl<'a> RecordStore for Esp<'a> {
    type Image = ImagePages<'a>;

    fn files(&self) -> &impl crate::slot_transaction::Store<Error = Status> {
        &self.files
    }

    fn read_image(&self, name: &str, expected: Option<usize>) -> Result<Self::Image, Status> {
        let mut buffer = [0; 96];
        let path = path(name, &mut buffer);
        // SAFETY: the constructor pairs the root with boot services.
        unsafe {
            let size = usize::try_from(self.files.0.size(path)?).map_err(|_| VOLUME_CORRUPTED)?;
            if size == 0 || size > MAX_IMAGE || expected.is_some_and(|expected| size != expected) {
                return Err(VOLUME_CORRUPTED);
            }
            let (base, size) = self.files.0.read_pages(self.boot, path)?;
            Ok(ImagePages {
                boot: self.boot,
                base,
                size,
                attached: false,
            })
        }
    }

    fn prepare_records(&self) -> Result<(), Status> {
        prepare_records(self.files.0)?;
        self.flush()
    }

    fn overwrite(&self, name: &str, bytes: &[u8; RECORD_SIZE]) -> Result<(), Status> {
        // SAFETY: the ESP root stays open.
        unsafe { self.files.0.overwrite(path(name, &mut [0; 96]), bytes) }
    }

    fn write_new_image(&self, name: &str, bytes: &[u8]) -> Result<(), Status> {
        // SAFETY: the ESP root and image bytes stay valid for the write.
        unsafe { self.files.0.write_new(path(name, &mut [0; 96]), bytes) }
    }

    fn visit_images(&self, visit: &mut dyn FnMut(&str)) -> Result<(), Status> {
        // SAFETY: the ESP root stays open.
        unsafe { self.files.0.visit_names(r"\EFI\BREENIX", visit) }
    }

    fn flush(&self) -> Result<(), Status> {
        // SAFETY: the device handle is the one paired with this root.
        unsafe { files::flush_volume(self.boot, self.device) }
    }
}

pub fn prepare_records(root: &Root) -> Result<(), Status> {
    // SAFETY: Root owns the firmware's directory handle.
    unsafe {
        for directory in [r"\EFI\BREENIX\A", r"\EFI\BREENIX\B"] {
            match root.size(directory) {
                Ok(_) => (),
                Err(NOT_FOUND) => root.ensure_directory(directory)?,
                Err(status) => return Err(status),
            }
        }
        for name in RECORD_PATHS {
            let mut buffer = [0; 96];
            let path = path(name, &mut buffer);
            match root.size(path) {
                Ok(size) if size == RECORD_SIZE as u64 => (),
                Ok(_) => root.write(path, &[0; RECORD_SIZE])?,
                Err(NOT_FOUND) => root.write_new(path, &[0; RECORD_SIZE])?,
                Err(status) => return Err(status),
            }
        }
    }
    Ok(())
}

pub struct ImagePages<'a> {
    boot: &'a BootServices,
    base: *mut u8,
    size: usize,
    attached: bool,
}

impl<'a> ImagePages<'a> {
    pub fn allocate(boot: &'a BootServices, size: usize) -> Result<Self, Status> {
        if size == 0 {
            return Err(INVALID_PARAMETER);
        }
        let mut address = 0;
        // SAFETY: the guard owns the resulting allocation.
        let pages = size.div_ceil(4096);
        let base = unsafe {
            ok((boot.allocate_pages)(
                ALLOCATE_ANY_PAGES,
                LOADER_DATA,
                pages,
                &mut address,
            ))?;
            let base = address as *mut u8;
            base.write_bytes(0, pages * 4096);
            base
        };
        Ok(Self {
            boot,
            base,
            size,
            attached: false,
        })
    }

    pub fn page_bytes(&mut self) -> &mut [u8] {
        // SAFETY: the guard owns these pages and the slice borrows the guard mutably.
        unsafe { core::slice::from_raw_parts_mut(self.base, self.size.div_ceil(4096) * 4096) }
    }

    /// The firmware may read the backing pages until ExitBootServices.
    pub fn retain_for_firmware(&mut self) {
        self.attached = true;
    }
}

impl AsRef<[u8]> for ImagePages<'_> {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: callers have initialized the image bytes before sharing them.
        unsafe { core::slice::from_raw_parts(self.base, self.size) }
    }
}

impl Drop for ImagePages<'_> {
    fn drop(&mut self) {
        // SAFETY: this guard owns the firmware allocation.
        if !self.attached {
            unsafe { (self.boot.free_pages)(self.base as u64, self.size.div_ceil(4096)) };
        }
    }
}
