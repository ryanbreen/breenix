//! x86 program-image entry points backed by the shared VFS exec loader.
//! ARM retains its interrupt-enable side effect on a boot directory error.

#[cfg(target_arch = "x86_64")]
use alloc::vec::Vec;
#[cfg(target_arch = "x86_64")]
use crate::exec_elf::{Image, MappedElf, Purpose};

/// Read the init ELF binary from the mounted root filesystem into its own
/// page table, with its physical absolute pathname.
///
/// Requires the root filesystem to be mounted.
/// Reading requires IRQ-driven VirtIO block completions, so the caller must
/// have interrupts hardware-enabled before calling this.
#[cfg(target_arch = "x86_64")]
pub fn read_init_from_ext2(path: &str) -> Result<(MappedElf, Vec<u8>), &'static str> {
    use crate::syscall::errno::{EISDIR, ENOENT, ENOEXEC};
    read_program(path).map_err(|errno| match errno {
        ENOENT => "init not found",
        EISDIR => "init is a directory",
        ENOEXEC => "not a loadable ELF executable for this machine",
        _ => "failed to read init",
    })
}

/// Read a program image for spawn or boot, resolving `path` through the
/// shared pathname resolver. Fails with the resolution's own errno, EISDIR for a
/// directory, EACCES when the caller may not execute the file, ENOEXEC when
/// it is not an ELF executable for this machine, or EINTR when a SIGKILL for
/// the caller arrives during the read. The image comes with its physical
/// absolute pathname.
#[cfg(target_arch = "x86_64")]
pub fn read_program(path: &str) -> Result<(MappedElf, Vec<u8>), i32> {
    let cred = crate::fs::permissions::Credentials::current(false).map_err(|errno| errno as i32)?;
    let (image, _, path) =
        crate::fs::vfs::exec::load_image(path, &cred, crate::syscall::exec::caller_killed, Purpose::Spawn)
            .map_err(|errno| errno.0)?;
    Ok((image.into_elf().map_err(|errno| errno.0)?, path))
}

/// The image exec runs from `path` (or its `#!` line), with the identity the
/// file's set-ID bits confer at exec and its physical absolute pathname.
#[cfg(target_arch = "x86_64")]
pub fn read_program_image(
    path: impl AsRef<[u8]>,
) -> Result<(Image, crate::process::credentials::ExecIdentity, Vec<u8>), i32> {
    let cred = crate::fs::permissions::Credentials::current(false).map_err(|errno| errno as i32)?;
    crate::fs::vfs::exec::load_image(path, &cred, crate::syscall::exec::caller_killed, Purpose::Exec)
        .map_err(|errno| errno.0)
}
