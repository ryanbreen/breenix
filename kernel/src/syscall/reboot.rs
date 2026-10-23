//! Final power transition, after PID 1 has stopped services and reaped users.
//! The ABI uses the published reboot magic words and command values.
use super::SyscallResult;
use crate::process::privilege::{self, Operation};

pub fn sys_reboot(magic: u64, magic2: u64, command: u64) -> SyscallResult {
    if let Err(errno) = privilege::require(Operation::Reboot) {
        return SyscallResult::Err(errno);
    }
    if magic != 0xfee1dead || magic2 != 672274793 || !matches!(command, 0x01234567 | 0x4321fedc) {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    // The service supervisor owns shutdown ordering; root's CLI asks it over
    // the credential-checked control socket rather than bypassing it.
    if !matches!(crate::syscall::handlers::sys_getpid(), SyscallResult::Ok(1)) {
        return SyscallResult::Err(super::errno::EPERM as u64);
    }
    {
        let manager = crate::process::manager();
        if manager.as_ref().is_none_or(|m| m.all_pids().into_iter().any(|pid|
            pid.as_u64() != 1 && m.get_process(pid).is_some_and(|p| !p.is_terminated()))) {
            return SyscallResult::Err(super::errno::EBUSY as u64);
        }
    }
    #[cfg(target_arch = "x86_64")]
    if command == 0x01234567 && !crate::arch_impl::x86_64::platform::reset_available() {
        return SyscallResult::Err(super::errno::ENODEV as u64);
    }
    #[cfg(target_arch = "x86_64")]
    let token = match power_token() {
        Ok(token) => token,
        Err(errno) => return SyscallResult::Err(errno),
    };
    if crate::fs::ext2::shutdown_filesystems().is_err() {
        return SyscallResult::Err(super::errno::EIO as u64);
    }
    let action = if command == 0x01234567 { "reboot" } else { "poweroff" };
    #[cfg(target_arch = "x86_64")]
    crate::serial_println!("[shutdown] requested {} token={}", action, core::str::from_utf8(&token).unwrap());
    #[cfg(target_arch = "aarch64")]
    crate::serial_println!("[shutdown] requested {}", action);
    crate::serial::flush_log();
    #[cfg(target_arch = "x86_64")]
    {
        if command == 0x01234567 {
            crate::arch_impl::x86_64::platform::reset();
        }
        // The server plan delegates S5 to the leased lab driver: no AML
        // interpreter runs in this kernel. It acts only on the receipt above.
        x86_64::instructions::interrupts::disable();
        loop { x86_64::instructions::hlt(); }
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // PSCI SYSTEM_RESET / SYSTEM_OFF; the persistent VM path is catch-up work.
        core::arch::asm!("hvc #0", in("x0") if command == 0x01234567 { 0x84000009u64 } else { 0x84000008u64 },
                         options(nomem, nostack));
        crate::hlt_loop();
    }
}

/// Read the protected host capability before making the filesystems read-only.
#[cfg(target_arch = "x86_64")]
fn power_token() -> Result<alloc::vec::Vec<u8>, u64> {
    let (mount, ino, _pin, _) = crate::fs::namei::resolve_file("/etc/breenix/power-token")?;
    let guard = mount.read()?;
    let fs = guard.as_ref().ok_or(super::errno::EIO as u64)?;
    let inode = fs.read_inode(ino).map_err(|_| super::errno::EIO as u64)?;
    if !inode.is_file() || inode.uid() != 0 || inode.i_mode & 0o7777 != 0o400 || inode.size() != 64 {
        return Err(super::errno::EACCES as u64);
    }
    let bytes = fs.read_file_content_coherent(ino, &inode).map_err(|_| super::errno::EIO as u64)?;
    if !bytes.iter().all(|b| b.is_ascii_hexdigit()) { return Err(super::errno::EINVAL as u64); }
    Ok(bytes)
}
