//! Files on a FAT volume through the firmware's simple file system and file protocols (UEFI 2.10 sections 13.4 and
//! 13.5): the boot manager's configuration and kernel slots on the EFI system partition, and the files an upgrade
//! writes there.
use core::ffi::c_void;
use core::ptr::null_mut;

use crate::uefi::*;

/// A volume's root directory, closed when dropped.
pub struct Root {
    pub dir: *mut File,
}

impl Drop for Root {
    fn drop(&mut self) {
        // SAFETY: the directory handle this root opened.
        unsafe {
            ((*self.dir).close)(self.dir);
        }
    }
}

/// An open file, closed when dropped unless it was deleted.
pub struct Open {
    file: *mut File,
}

impl Open {
    /// The open file's protocol, for reads at chosen positions.
    pub fn handle(&self) -> *mut File {
        self.file
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if !self.file.is_null() {
            // SAFETY: the file handle this guard opened.
            unsafe {
                ((*self.file).close)(self.file);
            }
        }
    }
}

/// The root directory of the volume on `handle`.
///
/// # Safety
/// `handle` must be a handle of this firmware.
pub unsafe fn root_of(boot: &BootServices, handle: Handle) -> Result<Root, Status> {
    let mut fs: *mut c_void = null_mut();
    ok((boot.handle_protocol)(handle, &SIMPLE_FILE_SYSTEM, &mut fs))?;
    let fs = fs.cast::<FileSystem>();
    let mut dir = null_mut();
    ok(((*fs).open_volume)(fs, &mut dir))?;
    Ok(Root { dir })
}

/// The root directory of the volume this image was loaded from, when it was loaded from a volume.
///
/// # Safety
/// `image` must be this application's handle.
pub unsafe fn own_root(boot: &BootServices, image: Handle) -> Result<Root, Status> {
    let mut loaded = null_mut();
    ok((boot.handle_protocol)(image, &LOADED_IMAGE, &mut loaded))?;
    root_of(boot, (*loaded.cast::<LoadedImage>()).device_handle)
}

impl Root {
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn open(&self, path: &str, mode: u64) -> Result<Open, Status> {
        let wide = Wide::<128>::new(path);
        let mut file = null_mut();
        ok(((*self.dir).open)(self.dir, &mut file, wide.as_ptr(), mode, 0))?;
        Ok(Open { file })
    }

    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn exists(&self, path: &str) -> bool {
        self.open(path, FILE_MODE_READ).is_ok()
    }

    /// Reads a whole small file into `buffer`; its length.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn read(&self, path: &str, buffer: &mut [u8]) -> Result<usize, Status> {
        let file = self.open(path, FILE_MODE_READ)?;
        let mut size = buffer.len();
        ok(((*file.file).read)(file.file, &mut size, buffer.as_mut_ptr().cast()))?;
        Ok(size)
    }

    /// Creates or replaces a file with `data`, then reads it back.
    ///
    /// # Safety
    /// The root's directory handle must still be open; `data` readable.
    pub unsafe fn write(&self, path: &str, data: &[u8]) -> Result<(), Status> {
        if self.exists(path) {
            self.delete(path)?;
        }
        let file = self.open(path, FILE_MODE_READ | FILE_MODE_WRITE | FILE_MODE_CREATE)?;
        let mut at = 0;
        while at < data.len() {
            let mut size = (data.len() - at).min(1 << 20);
            ok(((*file.file).write)(file.file, &mut size, data[at..].as_ptr().cast()))?;
            if size == 0 {
                return Err(DEVICE_ERROR);
            }
            at += size;
        }
        ok(((*file.file).flush)(file.file))?;
        drop(file);
        if self.size(path)? != data.len() as u64 {
            return Err(VOLUME_CORRUPTED);
        }
        let file = self.open(path, FILE_MODE_READ)?;
        let mut buffer = [0u8; 4096];
        let mut at = 0;
        while at < data.len() {
            let mut size = buffer.len().min(data.len() - at);
            ok(((*file.file).read)(file.file, &mut size, buffer.as_mut_ptr().cast()))?;
            if size == 0 || size > data.len() - at || buffer[..size] != data[at..at + size] {
                return Err(VOLUME_CORRUPTED);
            }
            at += size;
        }
        Ok(())
    }

    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn delete(&self, path: &str) -> Result<(), Status> {
        let mut file = self.open(path, FILE_MODE_READ | FILE_MODE_WRITE)?;
        let handle = core::mem::replace(&mut file.file, null_mut());
        // Delete closes the handle, whether or not it deletes the file.
        ok(((*handle).delete)(handle))
    }

    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn size(&self, path: &str) -> Result<u64, Status> {
        let file = self.open(path, FILE_MODE_READ)?;
        let mut info = [0u64; 64];
        let mut size = core::mem::size_of_val(&info);
        ok(((*file.file).get_info)(file.file, &FILE_INFO, &mut size, info.as_mut_ptr().cast()))?;
        Ok(info[1])
    }

    /// Renames a file within its directory: `to` is the new name without a directory.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn rename(&self, from: &str, to: &str) -> Result<(), Status> {
        let file = self.open(from, FILE_MODE_READ | FILE_MODE_WRITE)?;
        let mut info = [0u64; 64];
        let mut size = core::mem::size_of_val(&info);
        ok(((*file.file).get_info)(file.file, &FILE_INFO, &mut size, info.as_mut_ptr().cast()))?;
        let bytes = core::slice::from_raw_parts_mut(info.as_mut_ptr().cast::<u8>(), core::mem::size_of_val(&info));
        let name_units = to.len() + 1;
        if FILE_INFO_FIXED + 2 * name_units > bytes.len() {
            return Err(BAD_BUFFER_SIZE);
        }
        for (index, unit) in to.bytes().map(u16::from).chain([0]).enumerate() {
            bytes[FILE_INFO_FIXED + 2 * index..FILE_INFO_FIXED + 2 * index + 2].copy_from_slice(&unit.to_le_bytes());
        }
        let total = (FILE_INFO_FIXED + 2 * name_units) as u64;
        bytes[..8].copy_from_slice(&total.to_le_bytes());
        ok(((*file.file).set_info)(file.file, &FILE_INFO, total as usize, info.as_ptr().cast()))?;
        ok(((*file.file).flush)(file.file))
    }

    /// Reads a whole file into pages of its own; their address and the file's length.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn read_pages(&self, boot: &BootServices, path: &str) -> Result<(*mut u8, usize), Status> {
        let length = usize::try_from(self.size(path)?).map_err(|_| OUT_OF_RESOURCES)?;
        let pages = length.div_ceil(4096).max(1);
        let mut address = 0u64;
        ok((boot.allocate_pages)(ALLOCATE_ANY_PAGES, LOADER_DATA, pages, &mut address))?;
        let file = self.open(path, FILE_MODE_READ)?;
        let mut at = 0;
        while at < length {
            let mut size = (length - at).min(16 << 20);
            let status = ((*file.file).read)(file.file, &mut size, (address as *mut u8).add(at).cast());
            if status != SUCCESS || size == 0 {
                (boot.free_pages)(address, pages);
                return Err(if status == SUCCESS { VOLUME_CORRUPTED } else { status });
            }
            at += size;
        }
        Ok((address as *mut u8, length))
    }
}
