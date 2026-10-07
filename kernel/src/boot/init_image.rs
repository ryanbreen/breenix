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
    use crate::syscall::errno::{EISDIR, ENOENT};
    read_program(path).map_err(|errno| match errno {
        ENOENT => "init not found",
        EISDIR => "init is a directory",
        _ => "failed to read init",
    })
}

/// Read a program image for exec or spawn, resolving `path` as every
/// pathname is resolved. Fails with the resolution's own errno, EISDIR for a
/// directory, EACCES when the caller may not execute the file, or EINTR when
/// a SIGKILL for the caller arrives during the read.
#[cfg(target_arch = "x86_64")]
pub fn read_program(path: &str) -> Result<Vec<u8>, i32> {
    read_program_image(path).map(|(data, _)| data)
}

/// `read_program`, with the identity the file's set-ID bits confer at exec.
#[cfg(target_arch = "x86_64")]
pub fn read_program_image(
    path: &str,
) -> Result<(Vec<u8>, crate::process::credentials::ExecIdentity), i32> {
    use crate::syscall::errno::{EACCES, EIO, EISDIR};

    // Taken before the filesystem lock: who may execute the file.
    let cred = crate::fs::permissions::Credentials::current(false);

    // The handle holds the inode until its content is read.
    let (mount, inode_num, _held) =
        crate::fs::namei::resolve_file(path).map_err(|errno| errno as i32)?;
    let fs_guard = mount.read();
    let fs = fs_guard.as_ref().ok_or(EIO)?;

    let inode = fs.read_inode(inode_num).map_err(|_| EIO)?;

    if inode.is_dir() {
        return Err(EISDIR);
    }
    // A privileged caller still needs at least one execute bit (`permits`).
    if !cred.permits(&inode, 1) {
        return Err(EACCES);
    }

    let elf_data = fs
        .read_file_content_coherent_unless(inode_num, &inode, crate::syscall::exec::caller_killed)
        .map_err(|_| EIO)?
        .ok_or(crate::syscall::errno::EINTR)?;

    drop(fs_guard);

    Ok((elf_data, crate::process::credentials::ExecIdentity::of(&inode)))
}
