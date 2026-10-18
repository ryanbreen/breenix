//! The boot-target file: which PID 1 a production boot runs when the QEMU command
//! line does not say (docs/boot-path.md, "Boot modes").
//!
//! The runner writes `suite <id>`, `suite <id> case <category>/<case>` or `probe` to
//! `/etc/breenix/boot-target`, words separated by single spaces. It is
//! written onto a copy of the disk image by `./run.sh --parallels|--vmware --suite <id>`
//! (with `--case <category>/<case>` for one case) or `--probe`, and by
//! `BREENIX_BOOT_SUITE=<id>` in `docker/qemu/run-x86-gate.sh`; disks built by
//! `scripts/create_ext2_disk.sh` carry no boot target. QEMU's fw_cfg mode is read first;
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

/// A suite and an optional single `<category>/<case>` to watch.
pub struct SuiteTarget {
    pub id: String,
    pub case: Option<String>,
}

impl SuiteTarget {
    pub fn new(id: String, case: Option<String>) -> Result<Self, String> {
        if !is_suite_id(&id) {
            return Err(format!("{:?} is not a suite id", id));
        }
        if let Some(name) = &case {
            // Keep unknown names intact so the suite can list its valid IDs.
            // Exclude whitespace/newlines and fw_cfg's comma separator.
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./".contains(&b)) {
                return Err(format!("{:?} is not a case-id token", name));
            }
        }
        Ok(Self { id, case })
    }
}

/// A production PID 1 selected by the boot-target file.
pub enum Target {
    Default,
    Shell,
    Console { idle_minutes: u16, replug: bool },
    Probe,
    Suite(SuiteTarget),
}

/// Parse one `suite <id> [case <category>/<case>]` or `probe` line.
pub fn parse(text: &str) -> Result<Target, String> {
    let line = text.strip_suffix('\n').unwrap_or(text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.contains(['\n', '\r']) {
        return Err(String::from("it has more than one line"));
    }
    if line.trim() == "default" { return Ok(Target::Default); }
    if line.trim() == "shell" { return Ok(Target::Shell); }
    if line.trim() == "console" { return Ok(Target::Console { idle_minutes: 30, replug: false }); }
    if line.trim() == "probe" {
        return Ok(Target::Probe);
    }
    // Single spaces only, as the runner writes: a tab or a doubled space is refused.
    let words: alloc::vec::Vec<&str> = line.trim().split(' ').collect();
    match words.as_slice() {
        ["console", "replug"] => Ok(Target::Console { idle_minutes: 30, replug: true }),
        ["console", idle] if idle.starts_with("idle=") => {
            match idle[5..].parse::<u16>() {
                Ok(idle_minutes @ 1..=240) if idle[5..].bytes().all(|b| b.is_ascii_digit()) =>
                    Ok(Target::Console { idle_minutes, replug: false }),
                _ => Err(String::from("console idle must be 1 through 240 minutes")),
            }
        }
        ["suite", id] => SuiteTarget::new(String::from(*id), None).map(Target::Suite),
        ["suite", id, "case", case] => {
            SuiteTarget::new(String::from(*id), Some(String::from(*case))).map(Target::Suite)
        }
        _ => Err(format!("expected \"suite <id> [case <category>/<case>]\" or \"probe\", found {:?}", line)),
    }
}

static BOOT_INTENT: spin::Once<([u8; 256], usize)> = spin::Once::new();

/// Retain a small per-boot target payload without creating a RAM block device.
/// Parsing happens after allocator initialization, when PID 1 is selected.
pub fn capture(bytes: &[u8]) -> bool {
    let Some(line) = bytes.strip_prefix(b"BREENIX-BOOT 1\n") else { return false; };
    if line.is_empty() || line.len() > 256 { return false; }
    let mut saved = [0; 256];
    saved[..line.len()].copy_from_slice(line);
    BOOT_INTENT.call_once(|| (saved, line.len()));
    true
}

/// Read the boot target from the mounted ext2 root. `Ok(None)` when there is no
/// file (or no root filesystem); `Err` when the file exists but cannot be used.
pub fn read() -> Result<Option<Target>, String> {
    if let Some((bytes, len)) = BOOT_INTENT.get() {
        let text = core::str::from_utf8(&bytes[..*len]).map_err(|_| String::from("boot intent is not text"))?;
        return parse(text).map(Some);
    }
    let fs_guard = crate::fs::ext2::root_fs_read().map_err(String::from)?;
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
        .read_file_content_coherent(inode_num, &inode)
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
