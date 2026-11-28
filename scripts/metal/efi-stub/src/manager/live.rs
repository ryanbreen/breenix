//! The installed manager reads the staged payload from the BreenFS system
//! partition. Only the firmware writes the ESP, including its attempt record.
use core::ptr::{null, null_mut};

use metal_efi::breenfs_files::{Reader, BLOCK, NODE};
use metal_efi::files::{self, Root};
use metal_efi::live_sequence::{self, Media, Outcome};
use metal_efi::say;
use metal_efi::slot_store::{Esp, ImagePages};
use metal_efi::slot_transaction::Records;
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

pub unsafe fn reconcile(boot: &BootServices, image: Handle, root: &Root, records: &mut Records) {
    if let Err(status) = process(boot, image, root, records, false) {
        say!("live flash refused ({:#x})", status);
    }
}

pub unsafe fn start(
    boot: &BootServices,
    runtime: &RuntimeServices,
    image: Handle,
    root: &Root,
    records: &mut Records,
) {
    match process(boot, image, root, records, true) {
        Ok(Some(mut trial)) => {
            (boot.set_watchdog_timer)(300, 0, 0, null());
            let status = trial.mount(boot).map_or_else(
                |status| status,
                |volume| metal_efi::bootdisk::start_bootloader(boot, image, volume, "kernel slot"),
            );
            say!("live flash: candidate returned ({:#x}); restarting", status);
            (runtime.reset_system)(RESET_COLD, SUCCESS, 0, null());
        }
        Ok(None) => {}
        Err(status) => say!("live flash refused ({:#x}); booting CURRENT", status),
    }
}

unsafe fn process<'boot>(
    boot: &'boot BootServices,
    image: Handle,
    root: &Root,
    records: &mut Records,
    try_new: bool,
) -> Result<Option<TrialImage<'boot>>, Status> {
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
    let mut media = FirmwareMedia { boot, fs };
    let esp = Esp::new(root, boot, files::own_device(boot, image)?);
    let outcome = live_sequence::process(&esp, records, &mut media, try_new)?;
    slots::collect(&esp, records);
    match outcome {
        Outcome::Trial(m, pages) => {
            say!("live flash: trial {} {}", ascii(&m.token), ascii(&m.commit));
            Ok(Some(pages))
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

struct TrialImage<'boot> {
    pages: ImagePages<'boot>,
    volume: Option<*const u8>,
}

impl TrialImage<'_> {
    unsafe fn mount(&mut self, boot: &BootServices) -> Result<*const u8, Status> {
        if let Some(volume) = self.volume {
            return Ok(volume);
        }
        let volume = slots::mount(boot, &mut self.pages)?;
        self.volume = Some(volume);
        Ok(volume)
    }
}

impl AsRef<[u8]> for TrialImage<'_> {
    fn as_ref(&self) -> &[u8] {
        self.pages.as_ref()
    }
}

struct FirmwareMedia<'boot, 'fs, Read> {
    boot: &'boot BootServices,
    fs: Reader<'fs, Read>,
}

impl<'boot, Read: FnMut(u64, &mut [u8]) -> Result<(), ()>> Media
    for FirmwareMedia<'boot, '_, Read>
{
    type Image = TrialImage<'boot>;

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
        let mut pages = ImagePages::allocate(self.boot, size)?;
        self.fs
            .read_file(file, pages.page_bytes())
            .map_err(|_| VOLUME_CORRUPTED)?;
        Ok(TrialImage {
            pages,
            volume: None,
        })
    }

    fn root_flags(&mut self, name: &[u8]) -> Result<Option<u32>, Status> {
        self.fs.root_flags(name).map_err(|_| VOLUME_CORRUPTED)
    }

    fn trial_intent(&mut self, image: &mut Self::Image, bytes: &mut [u8]) -> Result<usize, Status> {
        // SAFETY: the image pages and the returned firmware path stay live.
        unsafe {
            let mut path = image.mount(self.boot)?;
            let mut handle = null_mut();
            ok((self.boot.locate_device_path)(
                &SIMPLE_FILE_SYSTEM,
                &mut path,
                &mut handle,
            ))?;
            files::root_of(self.boot, handle)?.read_bounded(r"\ramdisk", bytes)
        }
    }
}

fn ascii(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
}
