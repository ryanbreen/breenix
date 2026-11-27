//! The installed manager reads the staged payload from the BreenFS system
//! partition. Only the firmware writes the ESP, including its attempt record.
use core::ptr::{null, null_mut};

use metal_efi::breenfs_files::{BLOCK, NODE, Reader};
use metal_efi::files::{self, Root};
use metal_efi::live_flash::{self, Action, Manifest};
use metal_efi::say;
use metal_efi::slot_store::SlotStore;
use metal_efi::slot_transaction::{self, Store};
use metal_efi::uefi::*;

use crate::slots;

const CONFIG: &str = "\\EFI\\BREENIX\\LIVE.CFG";
const IMAGE: &str = "\\EFI\\BREENIX\\TRY.IMG";
const MARKER: &str = "LIVE.ID";
const NOTE: &str = "TRY.TXT";

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
    ok((boot.locate_handle_buffer)(BY_PROTOCOL, &BLOCK_IO, null(), &mut count, &mut handles))?;
    let mut found = None;
    let mut duplicate = false;
    for &handle in core::slice::from_raw_parts(handles, count) {
        if partition_signature(boot, handle) == Some(signature) {
            if found.replace(handle).is_some() { duplicate = true; }
        }
    }
    (boot.free_pool)(handles.cast());
    if duplicate { return Err(VOLUME_CORRUPTED); }
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
    boot: &BootServices, image: Handle, root: &Root, try_new: bool,
) -> Result<Option<(*mut u8, usize)>, Status> {
    let io = match system_disk(boot, root) {
        Ok(io) => io,
        Err(NOT_FOUND) => return Ok(None),
        Err(status) => return Err(status),
    };
    let media = &*(*io).media;
    let sector = u64::from(media.block_size);
    if media.logical_partition == 0 || media.media_present == 0 || sector == 0
        || BLOCK as u64 % sector != 0 || media.io_align > BLOCK as u32
    {
        return Err(UNSUPPORTED);
    }
    let ratio = BLOCK as u64 / sector;
    let total = media.last_block.checked_add(1).ok_or(VOLUME_CORRUPTED)? / ratio;
    let mut scratch = Aligned([0; NODE]);
    let mut fs = Reader::open(total, &mut scratch.0, |block, output| {
        ok(((*io).read_blocks)(io, media.media_id, block * ratio, output.len(), output.as_mut_ptr().cast()))
            .map_err(|_| ())
    }).map_err(|_| VOLUME_CORRUPTED)?;
    let mut small = Aligned([0; BLOCK]);
    let store = SlotStore(root);
    let mut attempted_text = [0; 256];
    let attempted = if store.exists(MARKER) {
        let length = store.read(MARKER, &mut attempted_text)?;
        Some(Manifest::parse(&attempted_text[..length]).ok_or(VOLUME_CORRUPTED)?)
    } else {
        None
    };
    let acknowledged = if let Ok(file) = fs.file(b"committed") {
        if file.size != 33 { return Err(VOLUME_CORRUPTED); }
        fs.read_file(file, &mut small.0).map_err(|_| VOLUME_CORRUPTED)?;
        if small.0[32] != b'\n' { return Err(VOLUME_CORRUPTED); }
        Some(live_flash::token(&small.0[..32]).ok_or(VOLUME_CORRUPTED)?)
    } else {
        None
    };

    if !try_new {
        if live_flash::choose(None, attempted.map(|m| m.token), acknowledged, store.exists("TRY.IMG"))
            == Action::Promote
        {
            let m = attempted.unwrap();
            let mut note = [0; 256];
            let length = store.read(NOTE, &mut note)?;
            if &note[..length] != slot_note(&m) {
                return Err(VOLUME_CORRUPTED);
            }
            if root.size(IMAGE)? != m.size as u64 { return Err(VOLUME_CORRUPTED); }
            let (base, size) = root.read_pages(boot, IMAGE)?;
            let verified = m.verifies(core::slice::from_raw_parts(base, size));
            (boot.free_pages)(base as u64, size.div_ceil(BLOCK));
            if !verified { return Err(VOLUME_CORRUPTED); }
            slot_transaction::promote_trial(&store)?;
            files::flush_volume(boot, files::own_device(boot, image)?)?;
        }
        if let Some(m) = attempted.filter(|m| acknowledged == Some(m.token)) {
            let mut current = [0; 256];
            if let Ok(size) = store.read("CURRENT.TXT", &mut current) {
                if current[..size] == slot_note(&m) {
                    say!("live flash: promoted {} {}", ascii(&m.token), ascii(&m.commit));
                }
            }
        }
        return Ok(None);
    }

    let Ok(file) = fs.file(b"manifest") else { return Ok(None); };
    if file.size > 255 { return Err(VOLUME_CORRUPTED); }
    let length = file.size;
    fs.read_file(file, &mut small.0).map_err(|_| VOLUME_CORRUPTED)?;
    let text = &small.0[..length];
    let m = Manifest::parse(text).ok_or(VOLUME_CORRUPTED)?;
    if live_flash::choose(Some(m.token), attempted.map(|m| m.token), None, false) != Action::Try {
        if acknowledged != Some(m.token) {
            say!("live flash: rollback {}", ascii(&m.token));
        }
        return Ok(None);
    }

    let file = fs.file(b"kernel.img").map_err(|_| NOT_FOUND)?;
    if file.size != m.size { return Err(VOLUME_CORRUPTED); }
    let pages = m.size.div_ceil(BLOCK);
    let mut address = 0;
    ok((boot.allocate_pages)(ALLOCATE_ANY_PAGES, LOADER_DATA, pages, &mut address))?;
    let base = address as *mut u8;
    let bytes = core::slice::from_raw_parts_mut(base, pages * BLOCK);
    let staged = (|| {
        fs.read_file(file, bytes).map_err(|_| VOLUME_CORRUPTED)?;
        if !m.verifies(&bytes[..m.size]) { return Err(VOLUME_CORRUPTED); }
        root.write(IMAGE, &bytes[..m.size])?;
        store.write(NOTE, &slot_note(&m))?;
        // The retained slot and its note must precede the durable attempt.
        let esp = files::own_device(boot, image)?;
        files::flush_volume(boot, esp)?;
        store.write(MARKER, text)?;
        files::flush_volume(boot, esp)?;
        Ok(())
    })();
    if let Err(status) = staged {
        (boot.free_pages)(address, pages);
        return Err(status);
    }
    say!("live flash: trial {} {}", ascii(&m.token), ascii(&m.commit));
    Ok(Some((base, m.size)))
}

fn ascii(bytes: &[u8]) -> &str { core::str::from_utf8(bytes).unwrap_or("") }

fn slot_note(manifest: &Manifest) -> [u8; 94] {
    let mut note = *b"commit 0000000000000000000000000000000000000000\ninstalled-by 00000000000000000000000000000000\n";
    note[7..47].copy_from_slice(&manifest.commit);
    note[61..93].copy_from_slice(&manifest.token);
    note
}
