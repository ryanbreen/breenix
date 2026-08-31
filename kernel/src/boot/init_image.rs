//! Read `/sbin/init` from the mounted ext2 root filesystem (x86_64).
//!
//! This is the x86_64 counterpart to aarch64's `read_init_from_ext2` in
//! `main_aarch64.rs`. The two are intentionally *not* unified into one shared
//! copy: aarch64's version has an aarch64-specific `Aarch64Cpu::enable_interrupts()`
//! side effect on its `is_dir` error path (it is called from contexts that have
//! already masked interrupts around a manual ERET sequence, and needs to undo
//! that on early return) that a shared copy would either have to strip out
//! (risking aarch64's interrupt-timing-sensitive boot sequence for no benefit,
//! since x86_64 never needs that call) or parameterize away. Forking this
//! dozen-line, read-only helper keeps `main_aarch64.rs` completely untouched
//! (#673 spec, Risks: "an acceptable fallback that keeps this arc's risk
//! surface minimal ... when unifying would touch more surface than forking a
//! dozen lines").

#[cfg(target_arch = "x86_64")]
use alloc::vec::Vec;

/// Read the init ELF binary from the mounted ext2 root filesystem.
///
/// Requires `kernel::fs::ext2::init_root_fs()` to have already succeeded.
/// Reading requires IRQ-driven VirtIO block completions, so the caller must
/// have interrupts hardware-enabled before calling this (matching every
/// other disk-backed load in `kernel_main_continue()`).
#[cfg(target_arch = "x86_64")]
pub fn read_init_from_ext2(path: &str) -> Result<Vec<u8>, &'static str> {
    let fs_guard = crate::fs::ext2::root_fs_read();
    let fs = fs_guard
        .as_ref()
        .ok_or("ext2 root filesystem not mounted")?;

    let inode_num = fs.resolve_path(path).map_err(|_| "init not found")?;

    let inode = fs
        .read_inode(inode_num)
        .map_err(|_| "failed to read inode")?;

    if inode.is_dir() {
        return Err("init is a directory");
    }

    let elf_data = fs
        .read_file_content(&inode)
        .map_err(|_| "failed to read init")?;

    drop(fs_guard);

    Ok(elf_data)
}
