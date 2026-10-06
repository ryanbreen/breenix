//! Exec preparation shared by both architectures. Nothing here changes the
//! caller's image: user copies, argument limits and interpreter lookup finish
//! before the process manager publishes a replacement address space.

use super::errno::{E2BIG, EACCES, EFAULT, EISDIR, ELOOP, ENOEXEC};
use alloc::{string::String, vec::Vec};

// sysconf(_SC_ARG_MAX) in musl uses a quarter of the soft RLIMIT_STACK,
// with a 128 KiB floor. Keep the default stack limit shared with getrlimit.
pub(crate) const DEFAULT_STACK_LIMIT: u64 = 8 * 1024 * 1024;
pub(crate) const ARG_MAX: usize = DEFAULT_STACK_LIMIT as usize / 4;

pub(crate) fn manager_errno(error: &str) -> u64 {
    match error {
        "exec blocked while CLONE_VM sibling shares old address space" => {
            super::errno::EAGAIN as u64
        }
        "exec arguments too large" => E2BIG as u64,
        _ => super::errno::ENOMEM as u64,
    }
}

pub(crate) struct Arguments {
    pub argv: Vec<Vec<u8>>,
    pub envp: Vec<Vec<u8>>,
}

impl Arguments {
    pub fn copy_from_user(path: &str, argv: u64, envp: u64) -> Result<Self, u64> {
        let mut budget = ARG_MAX;
        let mut args = Self {
            argv: copy_vector(argv, &mut budget)?,
            envp: copy_vector(envp, &mut budget)?,
        };
        // Retain the convenience ABI used by libbreenix::process::exec.
        // An explicitly supplied empty vector remains empty.
        if argv == 0 {
            args.argv.push(terminated(path.as_bytes()));
        }
        args.check_size()?;
        Ok(args)
    }

    pub fn check_size(&self) -> Result<(), u64> {
        argument_bytes(&self.argv, &self.envp).map(|_| ())
    }
}

fn terminated(bytes: &[u8]) -> Vec<u8> {
    let mut string = bytes.to_vec();
    string.push(0);
    string
}

fn copy_vector(mut vector: u64, budget: &mut usize) -> Result<Vec<Vec<u8>>, u64> {
    let mut strings = Vec::new();
    if vector == 0 {
        return Ok(strings);
    }
    loop {
        let pointer = super::userptr::copy_from_user(vector as *const u64)?;
        if pointer == 0 {
            return Ok(strings);
        }
        *budget = budget.checked_sub(8).ok_or(E2BIG as u64)?;
        let mut address = pointer;
        let mut string = Vec::new();
        loop {
            if *budget == 0 {
                return Err(E2BIG as u64);
            }
            // Do not read across a page boundary after a possible terminator.
            let mut bytes = [0u8; 256];
            let count = bytes
                .len()
                .min(4096 - (address as usize & 4095))
                .min(*budget);
            super::userptr::read_user_bytes(bytes.as_mut_ptr(), address, count)?;
            let length = bytes[..count]
                .iter()
                .position(|&byte| byte == 0)
                .map_or(count, |end| end + 1);
            string
                .try_reserve(length)
                .map_err(|_| super::errno::ENOMEM as u64)?;
            string.extend_from_slice(&bytes[..length]);
            *budget -= length;
            if string.last() == Some(&0) {
                break;
            }
            address = address.checked_add(length as u64).ok_or(EFAULT as u64)?;
        }
        strings
            .try_reserve(1)
            .map_err(|_| super::errno::ENOMEM as u64)?;
        strings.push(string);
        vector = vector.checked_add(8).ok_or(EFAULT as u64)?;
    }
}

fn argument_bytes(argv: &[Vec<u8>], envp: &[Vec<u8>]) -> Result<usize, u64> {
    let mut bytes = (argv.len() + envp.len())
        .checked_mul(8)
        .ok_or(E2BIG as u64)?;
    for string in argv.iter().chain(envp) {
        bytes = bytes.checked_add(string.len()).ok_or(E2BIG as u64)?;
    }
    if bytes > ARG_MAX {
        Err(E2BIG as u64)
    } else {
        Ok(bytes)
    }
}

/// Read scripts recursively, replacing argv[0] at each interpreter step. The
/// optional shebang argument is one string, including any internal whitespace.
/// Permission/lookup errors apply to each interpreter just as to the script.
pub(crate) fn read_image(
    path: &str,
    args: &mut Arguments,
    mut read: impl FnMut(&str) -> Result<Vec<u8>, i32>,
) -> Result<Vec<u8>, u64> {
    let mut path = String::from(path);
    for depth in 0..=4 {
        let data = read(&path).map_err(|errno| {
            if errno == EISDIR {
                EACCES as u64
            } else {
                errno as u64
            }
        })?;
        if !data.starts_with(b"#!") {
            validate_elf(&data)?;
            return Ok(data);
        }
        if depth == 4 {
            return Err(ELOOP as u64);
        }
        let end = data
            .iter()
            .position(|&b| b == b'\n')
            .ok_or(ENOEXEC as u64)?;
        let line = core::str::from_utf8(&data[2..end]).map_err(|_| ENOEXEC as u64)?;
        let line = line.trim_matches([' ', '\t']);
        let split = line.find([' ', '\t']).unwrap_or(line.len());
        let interpreter = &line[..split];
        if interpreter.is_empty() {
            return Err(ENOEXEC as u64);
        }
        let optional = line[split..].trim_matches([' ', '\t']);
        let mut argv = Vec::new();
        argv.push(terminated(interpreter.as_bytes()));
        if !optional.is_empty() {
            argv.push(terminated(optional.as_bytes()));
        }
        argv.push(terminated(path.as_bytes()));
        argv.extend(args.argv.drain(..).skip(1));
        args.argv = argv;
        args.check_size()?;
        path = String::from(interpreter);
    }
    Err(ELOOP as u64)
}

/// Check the complete ELF file structure before allocating or modifying an
/// address space. Mapping/allocation failures then remain ENOMEM rather than
/// being confused with an invalid executable's ENOEXEC.
fn validate_elf(data: &[u8]) -> Result<(), u64> {
    let bad = ENOEXEC as u64;
    if data.len() < 64 || &data[..4] != b"\x7fELF" || data[4..7] != [2, 1, 1] {
        return Err(bad);
    }
    let u16_at = |at| u16::from_le_bytes(data[at..at + 2].try_into().unwrap());
    let u64_at = |at| u64::from_le_bytes(data[at..at + 8].try_into().unwrap());
    #[cfg(target_arch = "x86_64")]
    let machine = 62;
    #[cfg(target_arch = "aarch64")]
    let machine = 183;
    if u16_at(16) != 2 || u16_at(18) != machine || u16_at(52) != 64 || u16_at(54) != 56 {
        return Err(bad);
    }
    let count = u16_at(56) as usize;
    let offset = u64_at(32) as usize;
    let end = offset
        .checked_add(count.checked_mul(56).ok_or(bad)?)
        .ok_or(bad)?;
    if count == 0 || end > data.len() {
        return Err(bad);
    }
    let mut executable_entry = false;
    for at in (offset..end).step_by(56) {
        let kind = u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        // Dynamic-linker images are not supported by either loader.
        if kind == 3 {
            return Err(bad);
        }
        if kind != 1 {
            continue;
        }
        let file_offset = u64_at(at + 8);
        let address = u64_at(at + 16);
        let file_size = u64_at(at + 32);
        let memory_size = u64_at(at + 40);
        let file_end = file_offset.checked_add(file_size).ok_or(bad)?;
        let memory_end = address.checked_add(memory_size).ok_or(bad)?;
        if file_size > memory_size
            || file_end > data.len() as u64
            || memory_end >= crate::memory::layout::USER_STACK_REGION_END
        {
            return Err(bad);
        }
        if data[at + 4] & 1 != 0 && (address..memory_end).contains(&u64_at(24)) {
            executable_entry = true;
        }
    }
    if executable_entry {
        Ok(())
    } else {
        Err(bad)
    }
}

/// Map enough initial stack for the full strings, pointers, unchanged auxv,
/// alignment and 64 KiB of runtime stack below them.
pub(crate) fn stack_size(argv: &[&[u8]], envp: &[&[u8]]) -> Result<usize, &'static str> {
    let mut size = (argv.len() + envp.len())
        .checked_mul(8)
        .ok_or("exec arguments too large")?;
    for string in argv.iter().chain(envp) {
        let length = string.len() + usize::from(string.last() != Some(&0));
        size = size.checked_add(length).ok_or("exec arguments too large")?;
    }
    if size > ARG_MAX {
        return Err("exec arguments too large");
    }
    // argc, two NULL pointers, seven auxv pairs, AT_RANDOM, and alignment.
    Ok((size + 8 * 17 + 16 + 23 + 64 * 1024 + 4095) & !4095)
}
