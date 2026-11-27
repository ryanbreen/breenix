//! The manager's files on the EFI system partition (docs/metal/README.md, "Disk layout" and "Slot actions"):
//! `MANAGER.CFG`, the kernel slots `CURRENT.IMG` and `PREVIOUS.IMG` with their `.TXT` notes, and `NEXT.*` while an
//! upgrade is staged.
use core::ptr::{null, null_mut};

use metal_efi::bootdisk;
use metal_efi::files::Root;
use metal_efi::say;
use metal_efi::uefi::*;

use crate::lab::Entry;

pub const DIR: &str = "\\EFI\\BREENIX\\";
const CONFIG_MAGIC: &str = "BREENIX-MANAGER 1";
pub const URL_MAX: usize = 512;
pub const PATH_MAX: usize = 512;

/// How the manager starts the test loop: loading the HTTP boot port's path with `LoadImage`, or setting `BootNext`
/// to the firmware's HTTP boot option and resetting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TestPath {
    LoadFile,
    BootNext,
}

pub struct Config {
    pub url: [u8; URL_MAX],
    pub url_len: usize,
    pub countdown: u32,
    /// The device path the firmware's HTTP boot loaded the install's stub from: the network port, its address and
    /// the boot URL. A site value, kept off COM1 and the screen.
    pub port: [u8; PATH_MAX],
    pub port_len: usize,
    pub http_option: Option<u16>,
    pub test_path: TestPath,
    rescue: Option<[[u8; 16]; 2]>,
}

impl Config {
    pub fn url(&self) -> &[u8] {
        &self.url[..self.url_len]
    }

    pub fn port(&self) -> &[u8] {
        &self.port[..self.port_len]
    }

    pub fn rescue_available(&self, partitions: impl IntoIterator<Item = [u8; 16]>) -> bool {
        let Some([system, home]) = self.rescue else { return false };
        let mut have_system = false;
        let mut have_home = false;
        for guid in partitions {
            have_system |= guid == system;
            have_home |= guid == home;
        }
        have_system && have_home
    }
}

fn unhex(text: &str, out: &mut [u8]) -> Option<usize> {
    if text.len() % 2 != 0 || text.len() / 2 > out.len() {
        return None;
    }
    for (index, pair) in text.as_bytes().chunks(2).enumerate() {
        out[index] = u8::from_str_radix(core::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(text.len() / 2)
}

/// `MANAGER.CFG`: the magic line, then `query <url>`, `countdown <seconds>`, `port <device path in hex>`,
/// `http-option <option number in hex>` and `test-path load-file|boot-next`.
pub fn parse_config(text: &[u8]) -> Option<Config> {
    let text = core::str::from_utf8(text).ok()?;
    let mut lines = text.lines();
    if lines.next()? != CONFIG_MAGIC {
        return None;
    }
    let mut config = Config {
        url: [0; URL_MAX],
        url_len: 0,
        countdown: 5,
        port: [0; PATH_MAX],
        port_len: 0,
        http_option: None,
        test_path: TestPath::BootNext,
        rescue: None,
    };
    for line in lines {
        match line.split_once(' ') {
            Some(("query", url)) if url.starts_with("http://") && url.len() < URL_MAX && url.is_ascii() => {
                config.url[..url.len()].copy_from_slice(url.as_bytes());
                config.url_len = url.len();
            }
            Some(("countdown", seconds)) => config.countdown = seconds.parse().ok().filter(|&s| (1..=60).contains(&s))?,
            Some(("port", hex)) => {
                config.port_len = unhex(hex, &mut config.port)?;
                if !is_whole_path(&config.port[..config.port_len]) {
                    return None;
                }
            }
            Some(("http-option", hex)) if hex.len() == 4 => config.http_option = Some(u16::from_str_radix(hex, 16).ok()?),
            Some(("test-path", "load-file")) => config.test_path = TestPath::LoadFile,
            Some(("test-path", "boot-next")) => config.test_path = TestPath::BootNext,
            Some(("rescue-partitions", pair)) if config.rescue.is_none() => {
                let (system, home) = pair.split_once(' ')?;
                let mut guids = [[0u8; 16]; 2];
                if unhex(system, &mut guids[0])? != 16 || unhex(home, &mut guids[1])? != 16
                    || guids[0] == guids[1] || guids.contains(&[0; 16])
                {
                    return None;
                }
                config.rescue = Some(guids);
            }
            _ if line.is_empty() => {}
            _ => return None,
        }
    }
    (config.url_len > 0 && config.port_len > 0).then_some(config)
}

/// A slot's note: the commit it holds.
#[derive(Clone, Copy)]
pub struct Note {
    pub commit: [u8; 40],
    pub present: bool,
}

impl Note {
    pub const ABSENT: Note = Note { commit: [b'?'; 40], present: false };

    pub fn short(&self) -> &str {
        core::str::from_utf8(&self.commit[..8]).unwrap_or("?")
    }

    /// The commit, or `empty` for a slot that holds no kernel.
    pub fn full(&self) -> &str {
        if !self.present {
            return "empty";
        }
        core::str::from_utf8(&self.commit).unwrap_or("?")
    }
}

pub fn slot_name(entry: Entry) -> &'static str {
    match entry {
        Entry::Current => "CURRENT",
        Entry::Previous => "PREVIOUS",
        Entry::Rescue => "RESCUE",
    }
}

fn path(name: &str, extension: &str, out: &mut [u8; 64]) -> usize {
    let mut len = 0;
    for part in [DIR, name, ".", extension] {
        out[len..len + part.len()].copy_from_slice(part.as_bytes());
        len += part.len();
    }
    len
}

/// The note of slot `name`, absent when its image is.
///
/// # Safety
/// `root` must be the EFI system partition's open root.
pub unsafe fn note(root: &Root, name: &str) -> Note {
    let mut image = [0u8; 64];
    let image_len = path(name, "IMG", &mut image);
    if !root.exists(core::str::from_utf8(&image[..image_len]).unwrap_or("")) {
        return Note::ABSENT;
    }
    let mut text_path = [0u8; 64];
    let len = path(name, "TXT", &mut text_path);
    let mut text = [0u8; 256];
    let Ok(size) = root.read(core::str::from_utf8(&text_path[..len]).unwrap_or(""), &mut text) else {
        return Note { commit: [b'?'; 40], present: true };
    };
    let mut note = Note { commit: [b'?'; 40], present: true };
    if let Ok(text) = core::str::from_utf8(&text[..size]) {
        for line in text.lines() {
            if let Some(commit) = line.strip_prefix("commit ") {
                if commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()) {
                    note.commit.copy_from_slice(commit.as_bytes());
                }
            }
        }
    }
    note
}

/// The rescue pair is selectable only while both of its GPT partitions are visible.
///
/// # Safety
/// `root` must be the EFI system partition's open root.
pub unsafe fn rescue_note(boot: &BootServices, root: &Root, config: &Config) -> Note {
    if config.rescue.is_none() {
        return Note::ABSENT;
    }
    let mut count = 0;
    let mut handles = null_mut();
    if (boot.locate_handle_buffer)(BY_PROTOCOL, &BLOCK_IO, null(), &mut count, &mut handles) != SUCCESS {
        return Note::ABSENT;
    }
    let signatures = (0..count).filter_map(|i| partition_signature(boot, *handles.add(i)));
    let available = config.rescue_available(signatures);
    (boot.free_pool)(handles.cast());
    if available {
        let note = note(root, slot_name(Entry::Rescue));
        if note.commit.iter().all(u8::is_ascii_hexdigit) {
            return note;
        }
    }
    Note::ABSENT
}

/// Boots slot `entry`: reads its image into memory, attaches it as a RAM disk and starts its bootloader. Returns only
/// when the slot could not be started, or its bootloader returned.
///
/// # Safety
/// `root` must be the EFI system partition's open root; `image` this application's handle.
pub unsafe fn boot(boot: &BootServices, image: Handle, root: &Root, entry: Entry) -> Status {
    let mut image_path = [0u8; 64];
    let len = path(slot_name(entry), "IMG", &mut image_path);
    let image_path = core::str::from_utf8(&image_path[..len]).unwrap_or("");
    let (base, size) = match root.read_pages(boot, image_path) {
        Ok(read) => read,
        Err(status) => {
            say!("could not read the {} slot ({:#x})", slot_name(entry), status);
            return status;
        }
    };
    say!("read the {} slot: {} bytes", slot_name(entry), size);
    boot_image(boot, image, base, size)
}

/// The verified image must remain readable for UEFI's disk protocols.
pub unsafe fn boot_image(boot: &BootServices, image: Handle, base: *mut u8, size: usize) -> Status {
    if !bootdisk::looks_like_disk(base, size) {
        say!("the slot is not a boot disk");
        return LOAD_ERROR;
    }
    let disk = match bootdisk::attach(boot, base, size, "kernel slot") {
        Ok(path) => path,
        Err(status) => return status,
    };
    let volume = match bootdisk::connect(boot, disk, "kernel slot") {
        Ok(path) => path,
        Err(status) => return status,
    };
    bootdisk::start_bootloader(boot, image, volume, "kernel slot")
}

use metal_efi::slot_store::SlotStore;

/// Recover an interrupted slot action before reading or booting a slot.
///
/// # Safety
/// `root` must be the EFI system partition's open root.
pub unsafe fn recover(root: &Root) -> Result<(), Status> {
    metal_efi::slot_transaction::recover(&SlotStore(root))
}

/// `NEXT` becomes `CURRENT` and the old current `PREVIOUS`. Repeating a commit with no staged slot leaves the slots as they are.
///
/// # Safety
/// `root` must be the EFI system partition's open root.
pub unsafe fn commit(root: &Root) -> Result<(), Status> {
    recover(root)?;
    if !note(root, "NEXT").present {
        say!("slot commit: nothing is staged; current {}", note(root, "CURRENT").full());
        return Ok(());
    }
    metal_efi::slot_transaction::begin(&SlotStore(root), false)?;
    say!(
        "slot commit: current {}, previous {}",
        note(root, "CURRENT").full(),
        note(root, "PREVIOUS").full()
    );
    Ok(())
}

/// `CURRENT` and `PREVIOUS` change places.
///
/// # Safety
/// `root` must be the EFI system partition's open root.
pub unsafe fn rollback(root: &Root) -> Result<(), Status> {
    recover(root)?;
    if !note(root, "PREVIOUS").present || !note(root, "CURRENT").present {
        say!("slot rollback refused: there is no previous slot to return to");
        return Err(NOT_FOUND);
    }
    metal_efi::slot_transaction::begin(&SlotStore(root), true)?;
    say!(
        "slot rollback: current {}, previous {}",
        note(root, "CURRENT").full(),
        note(root, "PREVIOUS").full()
    );
    Ok(())
}
