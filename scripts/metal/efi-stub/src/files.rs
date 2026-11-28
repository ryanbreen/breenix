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

    unsafe fn info(&self) -> Result<[u64; 64], Status> {
        let mut info = [0; 64];
        let mut size = core::mem::size_of_val(&info);
        ok(((*self.file).get_info)(self.file, &FILE_INFO, &mut size, info.as_mut_ptr().cast()))?;
        if size < FILE_INFO_FIXED {
            return Err(VOLUME_CORRUPTED);
        }
        Ok(info)
    }

    unsafe fn size(&self) -> Result<u64, Status> {
        Ok(self.info()?[1])
    }

    unsafe fn read_exact(&self, mut bytes: &mut [u8]) -> Result<(), Status> {
        while !bytes.is_empty() {
            let mut size = bytes.len();
            ok(((*self.file).read)(self.file, &mut size, bytes.as_mut_ptr().cast()))?;
            if size == 0 || size > bytes.len() {
                return Err(VOLUME_CORRUPTED);
            }
            bytes = &mut bytes[size..];
        }
        Ok(())
    }

    unsafe fn write_all(&self, mut data: &[u8]) -> Result<(), Status> {
        while !data.is_empty() {
            let mut size = data.len().min(1 << 20);
            ok(((*self.file).write)(self.file, &mut size, data.as_ptr().cast()))?;
            if size == 0 || size > data.len() {
                return Err(DEVICE_ERROR);
            }
            data = &data[size..];
        }
        ok(((*self.file).flush)(self.file))
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
    root_of(boot, own_device(boot, image)?)
}

/// The device handle recorded on the loaded image.
pub unsafe fn own_device(boot: &BootServices, image: Handle) -> Result<Handle, Status> {
    let mut loaded = null_mut();
    ok((boot.handle_protocol)(image, &LOADED_IMAGE, &mut loaded))?;
    Ok((*loaded.cast::<LoadedImage>()).device_handle)
}

pub unsafe fn block_io(boot: &BootServices, handle: Handle) -> Result<*mut BlockIo, Status> {
    let mut io = null_mut();
    ok((boot.handle_protocol)(handle, &BLOCK_IO, &mut io))?;
    Ok(io.cast())
}

pub unsafe fn flush_volume(boot: &BootServices, handle: Handle) -> Result<(), Status> {
    let io = block_io(boot, handle)?;
    ok(((*io).flush_blocks)(io))
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

    /// Reads a bounded file without accepting a truncated prefix.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn read_bounded(&self, path: &str, buffer: &mut [u8]) -> Result<usize, Status> {
        let file = self.open(path, FILE_MODE_READ)?;
        let length = usize::try_from(file.size()?).map_err(|_| BAD_BUFFER_SIZE)?;
        let output = buffer.get_mut(..length).ok_or(BAD_BUFFER_SIZE)?;
        file.read_exact(output)?;
        Ok(length)
    }

    /// Rewrites the bytes of an existing file without changing its length.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn overwrite(&self, path: &str, data: &[u8]) -> Result<(), Status> {
        let file = self.open(path, FILE_MODE_READ | FILE_MODE_WRITE)?;
        if file.size()? != data.len() as u64 {
            return Err(VOLUME_CORRUPTED);
        }
        ok(((*file.file).set_position)(file.file, 0))?;
        file.write_all(data)
    }

    /// Creates a fresh file, preserving an existing file at the same path.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn write_new(&self, path: &str, data: &[u8]) -> Result<(), Status> {
        match self.size(path) {
            Err(NOT_FOUND) => (),
            Ok(_) => return Err(ACCESS_DENIED),
            Err(status) => return Err(status),
        }
        self.create_and_check(path, data)
    }

    /// Releases a file's data in place, preserving its directory name.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn truncate(&self, path: &str) -> Result<bool, Status> {
        let file = self.open(path, FILE_MODE_READ | FILE_MODE_WRITE)?;
        let mut info = file.info()?;
        if info[1] == 0 {
            return Ok(false);
        }
        let size = usize::try_from(info[0]).map_err(|_| VOLUME_CORRUPTED)?;
        if !(FILE_INFO_FIXED + 2..=core::mem::size_of_val(&info)).contains(&size) {
            return Err(VOLUME_CORRUPTED);
        }
        info[1] = 0;
        ok(((*file.file).set_info)(
            file.file,
            &FILE_INFO,
            size,
            info.as_ptr().cast(),
        ))?;
        ok(((*file.file).flush)(file.file))?;
        Ok(true)
    }

    /// Opens or creates a directory.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn ensure_directory(&self, path: &str) -> Result<(), Status> {
        let wide = Wide::<128>::new(path);
        let mut handle = null_mut();
        ok(((*self.dir).open)(self.dir, &mut handle, wide.as_ptr(),
            FILE_MODE_READ | FILE_MODE_WRITE | FILE_MODE_CREATE, FILE_DIRECTORY))?;
        let file = Open { file: handle };
        if file.info()?[9] & FILE_DIRECTORY == 0 {
            return Err(VOLUME_CORRUPTED);
        }
        Ok(())
    }

    /// Visits the ASCII names returned by the firmware for a directory.
    ///
    /// # Safety
    /// The root's directory handle must still be open.
    pub unsafe fn visit_names(&self, path: &str, visit: &mut dyn FnMut(&str)) -> Result<(), Status> {
        let dir = self.open(path, FILE_MODE_READ)?;
        loop {
            // EFI_FILE_INFO has 8-byte alignment and a NUL-terminated CHAR16 name.
            let mut info = [0u64; 80];
            let mut length = core::mem::size_of_val(&info);
            ok(((*dir.file).read)(dir.file, &mut length, info.as_mut_ptr().cast()))?;
            if length == 0 {
                return Ok(());
            }
            if !(FILE_INFO_FIXED + 2..=core::mem::size_of_val(&info)).contains(&length) {
                return Err(VOLUME_CORRUPTED);
            }
            let data = core::slice::from_raw_parts(info.as_ptr().cast::<u8>(), length);
            let mut ascii = [0; 12];
            for (i, unit) in data[FILE_INFO_FIXED..].chunks_exact(2).enumerate() {
                if unit == [0, 0] {
                    visit(core::str::from_utf8(&ascii[..i]).map_err(|_| VOLUME_CORRUPTED)?);
                    break;
                }
                if i == ascii.len() || unit[1] != 0 || !unit[0].is_ascii() {
                    break;
                }
                ascii[i] = unit[0];
            }
        }
    }

    /// Creates or replaces a file with `data`, then reads it back.
    ///
    /// # Safety
    /// The root's directory handle must still be open; `data` readable.
    pub unsafe fn write(&self, path: &str, data: &[u8]) -> Result<(), Status> {
        if self.exists(path) {
            self.delete(path)?;
        }
        self.create_and_check(path, data)
    }

    unsafe fn create_and_check(&self, path: &str, data: &[u8]) -> Result<(), Status> {
        let file = self.open(path, FILE_MODE_READ | FILE_MODE_WRITE | FILE_MODE_CREATE)?;
        file.write_all(data)?;
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
        file.size()
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
        let file = self.open(path, FILE_MODE_READ)?;
        let length = usize::try_from(file.size()?).map_err(|_| OUT_OF_RESOURCES)?;
        let pages = length.div_ceil(4096).max(1);
        let mut address = 0u64;
        ok((boot.allocate_pages)(ALLOCATE_ANY_PAGES, LOADER_DATA, pages, &mut address))?;
        let mut at = 0;
        while at < length {
            let mut size = (length - at).min(16 << 20);
            let status = ((*file.file).read)(file.file, &mut size, (address as *mut u8).add(at).cast());
            if status != SUCCESS || size == 0 || size > length - at {
                (boot.free_pages)(address, pages);
                return Err(if status == SUCCESS { VOLUME_CORRUPTED } else { status });
            }
            at += size;
        }
        Ok((address as *mut u8, length))
    }
}
