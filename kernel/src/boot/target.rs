//! The boot-target file: which PID 1 a production boot runs when the QEMU command
//! line does not say (docs/boot-path.md, "Boot modes").
//!
//! `/etc/breenix/boot-target` on the ext2 root holds one line, `suite <id>`. It is
//! written onto a copy of the disk image by `./run.sh --parallels|--vmware --suite <id>`
//! and by `BREENIX_BOOT_SUITE=<id>` in `docker/qemu/run-x86-gate.sh`; disks built by
//! `scripts/create_ext2_disk.sh` carry none. QEMU's fw_cfg mode is read first;
//! without either, the default `/sbin/init` runs.

use alloc::format;
use alloc::string::String;

/// Where the boot target lives on the root filesystem.
pub const PATH: &str = "/etc/breenix/boot-target";

/// The largest boot-target file read. One `suite <id>` line is far shorter; a
/// larger file is refused before any of it is read.
const FILE_MAX: u64 = 256;

/// Whether `id` is a suite id: lowercase words of `a-z` and `0-9` joined by
/// single `-` (the rule the suite manifests and Vigil use).
pub fn is_suite_id(id: &str) -> bool {
    !id.is_empty()
        && id.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// The suite binary for a suite id.
pub fn suite_path(id: &str) -> String {
    format!("/sbin/suite-{}", id)
}

/// Parse the file's contents: one line, `suite <id>`, with an optional final
/// newline. Returns the suite id, or why the contents are not a boot target.
pub fn parse(text: &str) -> Result<String, String> {
    let line = text.strip_suffix('\n').unwrap_or(text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.contains(['\n', '\r']) {
        return Err(String::from("it has more than one line"));
    }
    match line.trim().split_once(' ') {
        Some(("suite", id)) if is_suite_id(id) => Ok(String::from(id)),
        Some(("suite", id)) => Err(format!("{:?} is not a suite id", id)),
        _ => Err(format!("expected \"suite <id>\", found {:?}", line)),
    }
}

/// Read the boot target from the mounted ext2 root. `Ok(None)` when there is no
/// file (or no root filesystem); `Err` when the file exists but cannot be used.
pub fn read() -> Result<Option<String>, String> {
    let fs_guard = crate::fs::ext2::root_fs_read();
    let Some(fs) = fs_guard.as_ref() else {
        return Ok(None);
    };
    let Ok(inode_num) = fs.resolve_path(PATH) else {
        return Ok(None);
    };
    let inode = fs
        .read_inode(inode_num)
        .map_err(|_| String::from("cannot read its inode"))?;
    if inode.is_dir() {
        return Err(String::from("it is a directory"));
    }
    let size = inode.size();
    if size > FILE_MAX {
        return Err(format!("it is {} bytes; expected one short line", size));
    }
    let bytes = fs
        .read_file_content(&inode)
        .map_err(|_| String::from("cannot read it"))?;
    drop(fs_guard);
    let text = core::str::from_utf8(&bytes).map_err(|_| String::from("it is not text"))?;
    parse(text).map(Some)
}

/// Check that `data` is an ELF executable this kernel can load as PID 1: a
/// little-endian 64-bit executable for this architecture whose program headers
/// and loadable segments lie inside the file. A boot mode whose binary fails
/// this check runs the default `/sbin/init` instead.
pub fn check_elf(data: &[u8]) -> Result<(), &'static str> {
    const HEADER: usize = 64;
    const PHDR: usize = 56;
    const PT_LOAD: u32 = 1;
    #[cfg(target_arch = "x86_64")]
    const MACHINE: u16 = 62;
    #[cfg(target_arch = "aarch64")]
    const MACHINE: u16 = 183;

    let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    let u64_at = |at: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&data[at..at + 8]);
        u64::from_le_bytes(bytes)
    };

    if data.len() < HEADER || data[0..4] != *b"\x7fELF" {
        return Err("not an ELF file");
    }
    if data[4] != 2 || data[5] != 1 {
        return Err("not a little-endian 64-bit ELF");
    }
    if !matches!(u16_at(16), 2 | 3) {
        return Err("not an executable");
    }
    if u16_at(18) != MACHINE {
        return Err("built for another architecture");
    }
    let phoff = u64_at(32) as usize;
    let phentsize = u16_at(54) as usize;
    let phnum = u16_at(56) as usize;
    if phentsize != PHDR || phnum == 0 {
        return Err("no usable program headers");
    }
    let table_end = phnum
        .checked_mul(PHDR)
        .and_then(|len| len.checked_add(phoff))
        .ok_or("program headers out of range")?;
    if table_end > data.len() {
        return Err("truncated: program headers past the end of the file");
    }
    let mut loads = 0;
    for index in 0..phnum {
        let at = phoff + index * PHDR;
        if u32_at(at) != PT_LOAD {
            continue;
        }
        loads += 1;
        let offset = u64_at(at + 8);
        let filesz = u64_at(at + 32);
        let memsz = u64_at(at + 40);
        if filesz > memsz {
            return Err("a segment is larger in the file than in memory");
        }
        match offset.checked_add(filesz) {
            Some(end) if end <= data.len() as u64 => {}
            _ => return Err("truncated: a segment runs past the end of the file"),
        }
    }
    if loads == 0 {
        return Err("no loadable segments");
    }
    Ok(())
}
