//! The boot-target file: which PID 1 a production boot runs when the QEMU command
//! line does not say (docs/boot-path.md, "Boot modes").
//!
//! `/etc/breenix/boot-target` on the ext2 root holds one line, `suite <id>`. It is
//! written onto a copy of the disk image by `./run.sh --parallels|--vmware --suite <id>`
//! and by `BREENIX_BOOT_SUITE=<id>` in `docker/qemu/run-x86-gate.sh`; disks built by
//! `scripts/create_ext2_disk.sh` carry none. ARM64 QEMU's fw_cfg mode is read first;
//! without either, the default `/sbin/init` runs.

use alloc::format;
use alloc::string::String;

/// Where the boot target lives on the root filesystem.
pub const PATH: &str = "/etc/breenix/boot-target";

/// The longest suite id accepted.
const ID_MAX: usize = 40;

/// Whether `id` is a suite id: lowercase words of `a-z` and `0-9` joined by single
/// `-`, at most 40 bytes (the rule the suite manifests and Vigil use).
pub fn is_suite_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_MAX
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

/// Parse the file's contents: one line, `suite <id>`. Returns the suite id, or why
/// the contents are not a boot target.
pub fn parse(text: &str) -> Result<String, String> {
    let line = text.trim();
    let mut words = line.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("suite"), Some(id), None) if is_suite_id(id) => Ok(String::from(id)),
        (Some("suite"), Some(id), None) => Err(format!("{:?} is not a suite id", id)),
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
    let bytes = fs
        .read_file_content(&inode)
        .map_err(|_| String::from("cannot read it"))?;
    drop(fs_guard);
    if bytes.len() > 256 {
        return Err(format!(
            "it is {} bytes; expected one short line",
            bytes.len()
        ));
    }
    let text = core::str::from_utf8(&bytes).map_err(|_| String::from("it is not text"))?;
    parse(text).map(Some)
}
