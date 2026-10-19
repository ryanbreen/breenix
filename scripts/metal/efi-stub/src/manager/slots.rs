//! The manager's files on the EFI system partition (docs/metal/README.md, "Disk layout" and "Slot actions"):
//! `MANAGER.CFG`, the kernel slots `CURRENT.IMG` and `PREVIOUS.IMG` with their `.TXT` notes, and `NEXT.*` while an
//! upgrade is staged.
use metal_efi::bootdisk;
use metal_efi::files::Root;
use metal_efi::say;
use metal_efi::uefi::*;

use crate::lab::Entry;

pub const DIR: &str = "\\EFI\\BREENIX\\";
const CONFIG_MAGIC: &str = "BREENIX-MANAGER 1";
pub const URL_MAX: usize = 512;

pub struct Config {
    pub url: [u8; URL_MAX],
    pub url_len: usize,
    pub countdown: u32,
}

impl Config {
    pub fn url(&self) -> &[u8] {
        &self.url[..self.url_len]
    }
}

/// `MANAGER.CFG`: the magic line, `query <url>` and `countdown <seconds>`.
pub fn parse_config(text: &[u8]) -> Option<Config> {
    let text = core::str::from_utf8(text).ok()?;
    let mut lines = text.lines();
    if lines.next()? != CONFIG_MAGIC {
        return None;
    }
    let mut config = Config { url: [0; URL_MAX], url_len: 0, countdown: 5 };
    for line in lines {
        match line.split_once(' ') {
            Some(("query", url)) if url.starts_with("http://") && url.len() < URL_MAX && url.is_ascii() => {
                config.url[..url.len()].copy_from_slice(url.as_bytes());
                config.url_len = url.len();
            }
            Some(("countdown", seconds)) => config.countdown = seconds.parse().ok().filter(|&s| (1..=60).contains(&s))?,
            _ if line.is_empty() => {}
            _ => return None,
        }
    }
    (config.url_len > 0).then_some(config)
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

    pub fn full(&self) -> &str {
        core::str::from_utf8(&self.commit).unwrap_or("?")
    }
}

pub fn slot_name(entry: Entry) -> &'static str {
    match entry {
        Entry::Current => "CURRENT",
        Entry::Previous => "PREVIOUS",
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

/// The note of slot `name` (`CURRENT`, `PREVIOUS` or `NEXT`), absent when its image is.
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
    if !bootdisk::looks_like_disk(base, size) {
        say!("the {} slot is not a boot disk", slot_name(entry));
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

unsafe fn rename_slot(root: &Root, from: &str, to: &str) -> Result<(), Status> {
    for extension in ["IMG", "TXT"] {
        let mut source = [0u8; 64];
        let len = path(from, extension, &mut source);
        let source = core::str::from_utf8(&source[..len]).unwrap_or("");
        let mut target = [0u8; 16];
        target[..to.len()].copy_from_slice(to.as_bytes());
        target[to.len()] = b'.';
        target[to.len() + 1..to.len() + 4].copy_from_slice(extension.as_bytes());
        let target = core::str::from_utf8(&target[..to.len() + 4]).unwrap_or("");
        if root.exists(source) {
            root.rename(source, target)?;
        }
    }
    Ok(())
}

unsafe fn delete_slot(root: &Root, name: &str) -> Result<(), Status> {
    for extension in ["IMG", "TXT"] {
        let mut file = [0u8; 64];
        let len = path(name, extension, &mut file);
        let file = core::str::from_utf8(&file[..len]).unwrap_or("");
        if root.exists(file) {
            root.delete(file)?;
        }
    }
    Ok(())
}

/// `NEXT` becomes `CURRENT` and the old current `PREVIOUS`. Repeating a commit with no staged slot leaves the slots as they are.
///
/// # Safety
/// `root` must be the EFI system partition's open root.
pub unsafe fn commit(root: &Root) -> Result<(), Status> {
    if !note(root, "NEXT").present {
        say!("slot commit: nothing is staged; current {}", note(root, "CURRENT").full());
        return Ok(());
    }
    if note(root, "CURRENT").present {
        delete_slot(root, "PREVIOUS")?;
        rename_slot(root, "CURRENT", "PREVIOUS")?;
    }
    rename_slot(root, "NEXT", "CURRENT")?;
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
    if !note(root, "PREVIOUS").present || !note(root, "CURRENT").present {
        say!("slot rollback refused: there is no previous slot to return to");
        return Err(NOT_FOUND);
    }
    delete_slot(root, "SWAP")?;
    rename_slot(root, "CURRENT", "SWAP")?;
    rename_slot(root, "PREVIOUS", "CURRENT")?;
    rename_slot(root, "SWAP", "PREVIOUS")?;
    say!(
        "slot rollback: current {}, previous {}",
        note(root, "CURRENT").full(),
        note(root, "PREVIOUS").full()
    );
    Ok(())
}
