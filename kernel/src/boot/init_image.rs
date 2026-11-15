//! x86 program-image entry points backed by the shared VFS exec loader.
//! ARM retains its interrupt-enable side effect on a boot directory error.

#[cfg(target_arch = "x86_64")]
use alloc::vec::Vec;

/// Read the init ELF binary from the mounted root filesystem, with its
/// physical absolute pathname.
///
/// Requires the root filesystem to be mounted.
/// Reading requires IRQ-driven VirtIO block completions, so the caller must
/// have interrupts hardware-enabled before calling this.
#[cfg(target_arch = "x86_64")]
pub fn read_init_from_ext2(path: &str) -> Result<(Vec<u8>, Vec<u8>), &'static str> {
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
/// a SIGKILL for the caller arrives during the read. The image comes with its
/// physical absolute pathname.
#[cfg(target_arch = "x86_64")]
pub fn read_program(path: &str) -> Result<(Vec<u8>, Vec<u8>), i32> {
    read_program_image(path).map(|(data, _, path)| (data, path))
}

/// `read_program`, with the identity the file's set-ID bits confer at exec.
#[cfg(target_arch = "x86_64")]
pub fn read_program_image(
    path: impl AsRef<[u8]>,
) -> Result<(Vec<u8>, crate::process::credentials::ExecIdentity, Vec<u8>), i32> {
    let cred = crate::fs::permissions::Credentials::current(false).map_err(|errno| errno as i32)?;
    crate::fs::vfs::exec::load_image(path, &cred, crate::syscall::exec::caller_killed)
        .map_err(|errno| errno.0)
}
