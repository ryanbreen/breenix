//! Final power transition, after PID 1 has stopped services and reaped users.
//! The ABI uses the published reboot magic words and command values.
use super::SyscallResult;
#[path = "power_token_validation.rs"]
mod power_token_validation;
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
    let caller = match crate::syscall::handlers::sys_getpid() {
        SyscallResult::Ok(pid) => crate::process::ProcessId::new(pid),
        SyscallResult::Err(errno) => return SyscallResult::Err(errno),
    };
    {
        let manager = crate::process::manager();
        let Some(manager) = manager.as_ref() else {
            return SyscallResult::Err(super::errno::EPERM as u64);
        };
        let Some(init) = manager.designated_init() else {
            return SyscallResult::Err(super::errno::EPERM as u64);
        };
        if caller != init {
            return SyscallResult::Err(super::errno::EPERM as u64);
        }
        if manager.all_pids().into_iter().any(|pid|
            pid != init && manager.get_process(pid).is_some_and(|p| !p.is_terminated())) {
            return SyscallResult::Err(super::errno::EBUSY as u64);
        }
    }
    #[cfg(target_arch = "x86_64")]
    if command == 0x01234567 && !crate::arch_impl::x86_64::platform::reset_available() {
        return SyscallResult::Err(super::errno::ENODEV as u64);
    }
    let token = match power_token() {
        Ok(token) => token,
        Err(errno) => return SyscallResult::Err(errno),
    };
    if crate::fs::ext2::shutdown_filesystems().is_err() {
        return SyscallResult::Err(super::errno::EIO as u64);
    }
    let action = if command == 0x01234567 { "reboot" } else { "poweroff" };
    if let Some(token) = token {
        crate::serial_println!("[shutdown] requested {} token={}", action, core::str::from_utf8(&token).unwrap());
    } else {
        crate::serial_println!("[shutdown] requested {}", action);
    }
    #[cfg(target_arch = "x86_64")]
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
    {
        crate::serial::drain_output();
        if command == 0x01234567 {
            crate::arch_impl::aarch64::psci::system_reset();
        } else {
            crate::arch_impl::aarch64::psci::system_off();
        }
        crate::hlt_loop();
    }
}

/// Read the protected host capability before making the filesystems read-only.
fn power_token() -> Result<Option<alloc::vec::Vec<u8>>, u64> {
    use power_token_validation::{validate, File, Invalid, Kind};
    let read = |path: &str, kind: Kind| -> Result<alloc::vec::Vec<u8>, u64> {
        let (mount, ino, _pin, _) = crate::fs::namei::resolve_file(path)?;
        let guard = mount.read()?;
        let fs = guard.as_ref().ok_or(super::errno::EIO as u64)?;
        let inode = fs.read_inode(ino).map_err(|_| super::errno::EIO as u64)?;
        // Reject untrusted metadata before reading its contents.
        let metadata = File { regular: inode.is_file(), uid: inode.uid() as u32,
            mode: inode.i_mode, size: inode.size() as u64, data: &[] };
        if validate(kind_ref(&kind), Some(metadata)) == Err(Invalid::Metadata) {
            return Err(super::errno::EACCES as u64);
        }
        let bytes = fs.read_file_content_coherent(ino, &inode).map_err(|_| super::errno::EIO as u64)?;
        let file = File { regular: inode.is_file(), uid: inode.uid() as u32,
            mode: inode.i_mode, size: inode.size() as u64, data: &bytes };
        validate(kind, Some(file)).map_err(|_| {
            #[cfg(target_arch = "x86_64")]
            { super::errno::EINVAL as u64 }
            #[cfg(target_arch = "aarch64")]
            { super::errno::EACCES as u64 }
        })?;
        Ok(bytes)
    };
    fn kind_ref(kind: &Kind) -> Kind {
        match kind { Kind::Token => Kind::Token, Kind::Nonce => Kind::Nonce }
    }
    let bytes = match read("/etc/breenix/power-token", Kind::Token) {
        Ok(bytes) => bytes,
        #[cfg(target_arch = "aarch64")]
        Err(errno) if errno == super::errno::ENOENT as u64 => return Ok(None),
        Err(errno) => return Err(errno),
    };
    let challenge = read("/run/breenix/shutdown-nonce", Kind::Nonce)?;
    let mut hash = crate::console::sha256::Sha256::new();
    hash.update(&bytes);
    hash.update(&challenge[..64]);
    Ok(Some(hash.finalize().iter().flat_map(|b| {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        [HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]
    }).collect()))
}
