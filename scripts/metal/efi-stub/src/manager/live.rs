//! The installed manager reads the staged payload from the BreenFS system
//! partition. Only the firmware writes the ESP, including its attempt record.
use core::ptr::{null, null_mut};

use metal_efi::breenfs_files::{Reader, BLOCK, NODE};
use metal_efi::files::{self, Root};
use metal_efi::live_sequence::{self, Media, Outcome};
use metal_efi::say;
use metal_efi::slot_store::SlotStore;
use metal_efi::uefi::*;

use crate::slots;

const CONFIG: &str = "\\EFI\\BREENIX\\LIVE.CFG";

#[repr(align(4096))]
struct Aligned<const N: usize>([u8; N]);

unsafe fn system_disk(boot: &BootServices, root: &Root) -> Result<*mut BlockIo, Status> {
    let mut signature = [0; 17];
    if root.size(CONFIG)? != 16 || root.read(CONFIG, &mut signature)? != 16 {
        return Err(VOLUME_CORRUPTED);
    }
    let signature: [u8; 16] = signature[..16].try_into().unwrap();
    let mut count = 0;
    let mut handles = null_mut();
    ok((boot.locate_handle_buffer)(
        BY_PROTOCOL,
        &BLOCK_IO,
        null(),
        &mut count,
        &mut handles,
    ))?;
    let mut found = None;
    let mut duplicate = false;
    for &handle in core::slice::from_raw_parts(handles, count) {
        if partition_signature(boot, handle) == Some(signature) {
            if found.replace(handle).is_some() {
                duplicate = true;
            }
        }
    }
    (boot.free_pool)(handles.cast());
    if duplicate {
        return Err(VOLUME_CORRUPTED);
    }
    files::block_io(boot, found.ok_or(NOT_FOUND)?)
}

/// Keep reparable stage errors on the normal kernel path. A slot transaction
/// whose journal has already been published must be recovered before booting.
pub unsafe fn reconcile(boot: &BootServices, image: Handle, root: &Root) -> Result<(), Status> {
    if let Err(status) = process(boot, image, root, false) {
        say!("live flash refused ({:#x})", status);
        slots::recover(root)?;
    }
    Ok(())
}

pub unsafe fn start(boot: &BootServices, runtime: &RuntimeServices, image: Handle, root: &Root) {
    match process(boot, image, root, true) {
        Ok(Some((base, size))) => {
            (boot.set_watchdog_timer)(300, 0, 0, null());
            let status = slots::boot_image(boot, image, base, size);
            say!("live flash: candidate returned ({:#x}); restarting", status);
            (runtime.reset_system)(RESET_COLD, SUCCESS, 0, null());
        }
        Ok(None) => {}
        Err(status) => say!("live flash refused ({:#x}); booting CURRENT", status),
    }
}

unsafe fn process(
    boot: &BootServices,
    image: Handle,
    root: &Root,
    try_new: bool,
) -> Result<Option<(*mut u8, usize)>, Status> {
    let io = match system_disk(boot, root) {
        Ok(io) => io,
        Err(NOT_FOUND) => return Ok(None),
        Err(status) => return Err(status),
    };
    let media = &*(*io).media;
    let sector = u64::from(media.block_size);
    if media.logical_partition == 0
        || media.media_present == 0
        || sector == 0
        || BLOCK as u64 % sector != 0
        || media.io_align > BLOCK as u32
    {
        return Err(UNSUPPORTED);
    }
    let ratio = BLOCK as u64 / sector;
    let total = media.last_block.checked_add(1).ok_or(VOLUME_CORRUPTED)? / ratio;
    let mut scratch = Aligned([0; NODE]);
    let fs = Reader::open(total, &mut scratch.0, |block, output| {
        ok(((*io).read_blocks)(
            io,
            media.media_id,
            block * ratio,
            output.len(),
            output.as_mut_ptr().cast(),
        ))
        .map_err(|_| ())
    })
    .map_err(|_| VOLUME_CORRUPTED)?;
    let mut media = FirmwareMedia {
        boot,
        image,
        root,
        fs,
    };
    match live_sequence::process(&SlotStore(root), &mut media, try_new)? {
        Outcome::Trial(m, pages) => {
            say!("live flash: trial {} {}", ascii(&m.token), ascii(&m.commit));
            Ok(Some(pages.into_raw()))
        }
        Outcome::Promoted(m) => {
            say!(
                "live flash: promoted {} {}",
                ascii(&m.token),
                ascii(&m.commit)
            );
            Ok(None)
        }
        Outcome::Rollback(token) => {
            say!("live flash: rollback {}", ascii(&token));
            Ok(None)
        }
        Outcome::StaleBase(token) => {
            say!("live flash: stale base for {}", ascii(&token));
            Ok(None)
        }
        Outcome::Current => Ok(None),
    }
}

struct ImagePages<'a> {
    boot: &'a BootServices,
    base: *mut u8,
    size: usize,
}

impl ImagePages<'_> {
    fn into_raw(self) -> (*mut u8, usize) {
        let raw = (self.base, self.size);
        // The boot disk keeps these pages through the bootloader handoff.
        core::mem::forget(self);
        raw
    }
}

impl AsRef<[u8]> for ImagePages<'_> {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: these pages belong to this guard and the image was read in full.
        unsafe { core::slice::from_raw_parts(self.base, self.size) }
    }
}

impl Drop for ImagePages<'_> {
    fn drop(&mut self) {
        // SAFETY: this guard owns the firmware allocation.
        unsafe { (self.boot.free_pages)(self.base as u64, self.size.div_ceil(BLOCK)) };
    }
}

struct FirmwareMedia<'boot, 'fs, Read> {
    boot: &'boot BootServices,
    image: Handle,
    root: &'boot Root,
    fs: Reader<'fs, Read>,
}

impl<'boot, Read: FnMut(u64, &mut [u8]) -> Result<(), ()>> Media
    for FirmwareMedia<'boot, '_, Read>
{
    type Image = ImagePages<'boot>;

    fn read_record(&mut self, name: &[u8], bytes: &mut [u8]) -> Result<Option<usize>, Status> {
        let Ok(file) = self.fs.file(name) else {
            return Ok(None);
        };
        let size = file.size;
        if size > bytes.len() {
            return Err(VOLUME_CORRUPTED);
        }
        let mut small = Aligned([0; BLOCK]);
        self.fs
            .read_file(file, &mut small.0)
            .map_err(|_| VOLUME_CORRUPTED)?;
        bytes[..size].copy_from_slice(&small.0[..size]);
        Ok(Some(size))
    }

    fn staged_image(&mut self, size: usize) -> Result<Self::Image, Status> {
        let file = self.fs.file(b"kernel.img").map_err(|_| NOT_FOUND)?;
        if file.size != size {
            return Err(VOLUME_CORRUPTED);
        }
        let pages = size.div_ceil(BLOCK);
        let mut address = 0;
        // SAFETY: the firmware allocation is owned by the guard from here on;
        // only the reader mutates its backing pages before the image is used.
        unsafe {
            ok((self.boot.allocate_pages)(
                ALLOCATE_ANY_PAGES,
                LOADER_DATA,
                pages,
                &mut address,
            ))?;
            let image = ImagePages {
                boot: self.boot,
                base: address as *mut u8,
                size,
            };
            let bytes = core::slice::from_raw_parts_mut(image.base, pages * BLOCK);
            self.fs
                .read_file(file, bytes)
                .map_err(|_| VOLUME_CORRUPTED)?;
            Ok(image)
        }
    }

    fn retained_image(&self, next: bool, size: usize) -> Result<Self::Image, Status> {
        let path = if next {
            r"\EFI\BREENIX\NEXT.IMG"
        } else {
            r"\EFI\BREENIX\TRY.IMG"
        };
        // SAFETY: the adapter borrows an open ESP root and boot services.
        unsafe {
            if self.root.size(path)? != size as u64 {
                return Err(VOLUME_CORRUPTED);
            }
            let (base, size) = self.root.read_pages(self.boot, path)?;
            Ok(ImagePages {
                boot: self.boot,
                base,
                size,
            })
        }
    }

    fn flush(&self) -> Result<(), Status> {
        // SAFETY: the adapter holds this application's image and boot services.
        unsafe { files::flush_volume(self.boot, files::own_device(self.boot, self.image)?) }
    }
}

fn ascii(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
}
