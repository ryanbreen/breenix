//! Copy exec inputs and resolve interpreters before publishing a new image.

use super::errno::{E2BIG, EACCES, EFAULT, EISDIR, ELOOP, ENOEXEC, ENOMEM};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::ops::Range;

// Reserve runtime stack below the arguments within the demand-growth cap.
pub(crate) const DEFAULT_STACK_LIMIT: u64 = crate::memory::layout::MAX_USER_STACK_SIZE;
pub(crate) const ARG_MAX: usize = DEFAULT_STACK_LIMIT as usize / 4;

pub(crate) fn manager_errno(error: &str) -> u64 {
    match error {
        "exec blocked while CLONE_VM sibling shares old address space"
        | "exec credentials changed while its image was prepared" => super::errno::EAGAIN as u64,
        "exec arguments too large" => E2BIG as u64,
        _ => ENOMEM as u64,
    }
}

// Strings share one allocation; ranges are offsets within that owned buffer.
pub(crate) struct Arguments {
    bytes: Vec<u8>,
    argv: Vec<Range<usize>>,
    envp: Vec<Range<usize>>,
}

impl Arguments {
    fn empty() -> Self {
        Self {
            bytes: Vec::new(),
            argv: Vec::new(),
            envp: Vec::new(),
        }
    }

    pub fn copy_from_user(path: &str, argv: u64, envp: u64) -> Result<Self, u64> {
        let mut args = Self::empty();
        let mut budget = ARG_MAX;
        args.argv = copy_vector(argv, &mut budget, &mut args.bytes)?;
        args.envp = copy_vector(envp, &mut budget, &mut args.bytes)?;
        // libbreenix's no-argv convenience call uses the pathname as argv[0].
        if argv == 0 {
            args.argv.try_reserve(1).map_err(|_| ENOMEM as u64)?;
            let range = args.append(path.as_bytes())?;
            args.argv.push(range);
        }
        args.check_size()?;
        Ok(args)
    }

    fn append(&mut self, value: &[u8]) -> Result<Range<usize>, u64> {
        let start = self.bytes.len();
        self.bytes
            .try_reserve(value.len() + 1)
            .map_err(|_| ENOMEM as u64)?;
        self.bytes.extend_from_slice(value);
        self.bytes.push(0);
        Ok(start..self.bytes.len())
    }

    pub fn check_size(&self) -> Result<(), u64> {
        let size = (self.argv.len() + self.envp.len())
            .checked_mul(8)
            .and_then(|n| n.checked_add(self.bytes.len()))
            .ok_or(E2BIG as u64)?;
        if size > ARG_MAX {
            Err(E2BIG as u64)
        } else {
            Ok(())
        }
    }

    pub fn slices(&self) -> Result<(Vec<&[u8]>, Vec<&[u8]>), u64> {
        let mut argv = Vec::new();
        let mut envp = Vec::new();
        argv.try_reserve_exact(self.argv.len())
            .map_err(|_| ENOMEM as u64)?;
        envp.try_reserve_exact(self.envp.len())
            .map_err(|_| ENOMEM as u64)?;
        argv.extend(self.argv.iter().map(|range| &self.bytes[range.clone()]));
        envp.extend(self.envp.iter().map(|range| &self.bytes[range.clone()]));
        Ok((argv, envp))
    }

    fn interpreter(&mut self, interpreter: &[u8], optional: &[u8], path: &[u8]) -> Result<(), u64> {
        let mut expanded = Self::empty();
        expanded
            .argv
            .try_reserve_exact(self.argv.len() + 3)
            .map_err(|_| ENOMEM as u64)?;
        expanded
            .envp
            .try_reserve_exact(self.envp.len())
            .map_err(|_| ENOMEM as u64)?;
        let range = expanded.append(interpreter)?;
        expanded.argv.push(range);
        if !optional.is_empty() {
            let range = expanded.append(optional)?;
            expanded.argv.push(range);
        }
        let range = expanded.append(path)?;
        expanded.argv.push(range);
        for range in self.argv.iter().skip(1) {
            let range = expanded.append(&self.bytes[range.start..range.end - 1])?;
            expanded.argv.push(range);
        }
        for range in &self.envp {
            let range = expanded.append(&self.bytes[range.start..range.end - 1])?;
            expanded.envp.push(range);
        }
        expanded.check_size()?;
        *self = expanded;
        Ok(())
    }
}

fn copy_vector(
    mut vector: u64,
    budget: &mut usize,
    bytes: &mut Vec<u8>,
) -> Result<Vec<Range<usize>>, u64> {
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
        let start = bytes.len();
        let mut address = pointer;
        loop {
            if *budget == 0 {
                return Err(E2BIG as u64);
            }
            // Never read past a page containing a possible terminator.
            let mut chunk = [0u8; 256];
            let count = chunk
                .len()
                .min(4096 - (address as usize & 4095))
                .min(*budget);
            super::userptr::read_user_bytes(chunk.as_mut_ptr(), address, count)?;
            let length = chunk[..count]
                .iter()
                .position(|&byte| byte == 0)
                .map_or(count, |end| end + 1);
            bytes.try_reserve(length).map_err(|_| ENOMEM as u64)?;
            bytes.extend_from_slice(&chunk[..length]);
            *budget -= length;
            if bytes.last() == Some(&0) {
                break;
            }
            address = address.checked_add(length as u64).ok_or(EFAULT as u64)?;
        }
        strings.try_reserve(1).map_err(|_| ENOMEM as u64)?;
        strings.push(start..bytes.len());
        vector = vector.checked_add(8).ok_or(EFAULT as u64)?;
    }
}

pub(crate) fn copy_string(value: &str) -> Result<String, u64> {
    let mut result = String::new();
    result
        .try_reserve_exact(value.len())
        .map_err(|_| ENOMEM as u64)?;
    result.push_str(value);
    Ok(result)
}

pub(crate) fn try_box<T>(value: T) -> Result<Box<T>, &'static str> {
    let layout = core::alloc::Layout::new::<T>();
    if layout.size() == 0 {
        return Ok(Box::new(value));
    }
    // SAFETY: allocate storage for T, initialize it once, then transfer ownership.
    unsafe {
        let pointer = alloc::alloc::alloc(layout) as *mut T;
        if pointer.is_null() {
            return Err("exec allocation failed");
        }
        pointer.write(value);
        Ok(Box::from_raw(pointer))
    }
}

/// Whether a SIGKILL is pending for the calling thread's process. An exec or
/// spawn reading a program image for it can stop: the process dies at this
/// syscall's return whatever the call does (`KillCustody::enter_syscall`).
/// Never waits for the process manager, so it may be asked with a filesystem
/// lock held; while the manager is busy the answer is no.
pub(crate) fn caller_killed() -> bool {
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return false;
    };
    let Some(guard) = crate::process::try_manager() else {
        return false;
    };
    guard
        .as_ref()
        .and_then(|manager| manager.find_process_by_thread(thread_id))
        .is_some_and(|(_, process)| {
            process.signals.pending & crate::signal::constants::sig_mask(crate::signal::constants::SIGKILL)
                != 0
        })
}

/// Expand interpreter scripts, keeping the optional argument as one string.
/// `read` returns a file's bytes and what else its loader learned about it;
/// that of the ELF image finally loaded is returned with it, so a script's
/// own set-ID bits confer nothing and its interpreter's do.
pub(crate) fn read_image<I>(
    path: &str,
    args: &mut Arguments,
    mut read: impl FnMut(&str) -> Result<(Vec<u8>, I), i32>,
) -> Result<(Vec<u8>, I), u64> {
    let mut path = copy_string(path)?;
    for depth in 0..=5 {
        let (data, identity) = read(&path).map_err(|errno| {
            if errno == EISDIR {
                EACCES as u64
            } else {
                errno as u64
            }
        })?;
        if !data.starts_with(b"#!") {
            validate_elf(&data)?;
            return Ok((data, identity));
        }
        if depth == 5 {
            return Err(ELOOP as u64);
        }
        let end = data.iter().position(|&b| b == b'\n').unwrap_or(data.len());
        // Pathnames in the filesystem API are UTF-8; invalid bytes cannot resolve.
        let line = core::str::from_utf8(&data[2..end]).map_err(|_| super::errno::ENOENT as u64)?;
        let line = line.trim_matches([' ', '\t']);
        let split = line.find([' ', '\t']).unwrap_or(line.len());
        let interpreter = &line[..split];
        if interpreter.is_empty() {
            return Err(ENOEXEC as u64);
        }
        let optional = line[split..].trim_matches([' ', '\t']);
        // The exec convenience ABI searches /bin for bare names. Pass the
        // loaded script's path, so its interpreter does not resolve it in cwd.
        if !path.contains('/') {
            let mut resolved = String::new();
            resolved
                .try_reserve_exact(5 + path.len())
                .map_err(|_| ENOMEM as u64)?;
            resolved.push_str("/bin/");
            resolved.push_str(&path);
            path = resolved;
        }
        args.interpreter(interpreter.as_bytes(), optional.as_bytes(), path.as_bytes())?;
        path = copy_string(interpreter)?;
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
        // Exclusive: a segment may fill the last user page.
        let memory_end = address.checked_add(memory_size).ok_or(bad)?;
        if file_size > memory_size
            || file_end > data.len() as u64
            || memory_end > crate::memory::layout::USER_STACK_REGION_END
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
    // argc, two NULL pointers, twelve auxv pairs, AT_RANDOM, and alignment.
    Ok((size + 8 * 27 + 16 + 23 + 64 * 1024 + 4095) & !4095)
}
