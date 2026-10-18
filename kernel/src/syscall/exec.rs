//! Copy exec inputs and resolve interpreters before publishing a new image.

use super::errno::{E2BIG, EACCES, EFAULT, EISDIR, ELOOP, ENOEXEC, ENOMEM};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::ops::Range;

// Reserve runtime stack below the arguments within the demand-growth cap.
pub(crate) const DEFAULT_STACK_LIMIT: u64 = crate::memory::layout::MAX_USER_STACK_SIZE;
pub(crate) const ARG_MAX: usize = DEFAULT_STACK_LIMIT as usize / 4;

pub(crate) fn manager_errno(error: &str) -> u64 {
    match error {
        "exec credentials changed while its image was prepared" => super::errno::EAGAIN as u64,
        "exec arguments too large" => E2BIG as u64,
        _ => ENOMEM as u64,
    }
}

/// No waker is registered on this queue: an exec waiting for the other threads
/// of its process to end sleeps on it one interval at a time and checks again.
static OTHER_THREADS_WAIT: crate::task::waitqueue::WaitQueueHead = crate::task::waitqueue::WaitQueueHead::new();

/// POSIX exec by a thread of a multithreaded process (S2), before the new
/// image is committed: end the process's other threads, wait until they are
/// gone (`ProcessManager::exec_waits_for_group`), and make the calling row
/// the process's (`ProcessManager::exec_take_over_group`). Returns the
/// process ID the caller's row now has: its thread group's ID, which is
/// `pid` when the caller already led its process or shares no thread group.
/// Rows that share the address space without sharing the thread group
/// (CLONE_VM only, vfork) are left running; the commit gives the caller a new
/// address space and leaves them the old one.
///
/// The wait sleeps a millisecond at a time, so lower-priority threads run to
/// their end, and drives the deferred reclaim that retires the leader's row.
/// A signal ends it with the errno its wait decided. When another thread's
/// exec or an exit of the process marked this thread first, it kills no
/// thread and returns EINTR; the pending SIGKILL ends the thread at its
/// return from the system call.
pub(crate) fn end_other_threads(pid: crate::process::ProcessId) -> Result<crate::process::ProcessId, u64> {
    use crate::ipc::wait::{wait_on, IpcDeadline, WaitEnd};
    const INTERVAL_NS: u64 = 1_000_000;
    // The existing kill path defers peers inside a system call and protects
    // their roots until their last dispatch ends.
    if !crate::signal::delivery::terminate_peers_for_exec(pid) {
        return Err(super::errno::EINTR as u64);
    }
    loop {
        let ended = crate::process::with_process_manager(|manager| {
            (!manager.exec_waits_for_group(pid)).then(|| manager.exec_take_over_group(pid))
        })
        .ok_or(super::errno::ESRCH as u64)?;
        match ended {
            Some(None) => return Ok(pid),
            Some(Some((process, leader_row))) => {
                drop(leader_row);
                if let Some(thread) = crate::task::scheduler::current_thread_id() {
                    crate::task::scheduler::adopt_exec_owner(thread, process.as_u64());
                }
                return Ok(process);
            }
            None => {}
        }
        crate::task::process_task::reclaim_deferred_process_resources();
        let (sec, nsec) = crate::time::get_monotonic_time_ns();
        let at = sec.saturating_mul(1_000_000_000).saturating_add(nsec).saturating_add(INTERVAL_NS);
        if let WaitEnd::Interrupted(errno) = wait_on(&OTHER_THREADS_WAIT, &IpcDeadline::Monotonic(at), || true) {
            return Err(errno);
        }
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

    pub fn copy_from_user(path: impl AsRef<[u8]>, argv: u64, envp: u64) -> Result<Self, u64> {
        let mut args = Self::empty();
        let mut budget = ARG_MAX;
        args.argv = copy_vector(argv, &mut budget, &mut args.bytes)?;
        args.envp = copy_vector(envp, &mut budget, &mut args.bytes)?;
        // libbreenix's no-argv convenience call uses the pathname as argv[0].
        if argv == 0 {
            args.argv.try_reserve(1).map_err(|_| ENOMEM as u64)?;
            let range = args.append(path.as_ref())?;
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
            process.signals.pending() & crate::signal::constants::sig_mask(crate::signal::constants::SIGKILL)
                != 0
        })
}

/// A pathname as display text, such as a process name: invalid UTF-8 becomes
/// U+FFFD. The resolver and interpreter arguments keep the raw bytes.
pub(crate) fn display_path(mut path: &[u8]) -> Result<String, u64> {
    let mut text = String::new();
    text.try_reserve_exact(path.len().checked_mul(3).ok_or(ENOMEM as u64)?).map_err(|_| ENOMEM as u64)?;
    while !path.is_empty() {
        match core::str::from_utf8(path) {
            Ok(valid) => { text.push_str(valid); break; }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(core::str::from_utf8(&path[..valid]).map_err(|_| EFAULT as u64)?);
                text.push('\u{fffd}');
                path = &path[valid + error.error_len().unwrap_or(path.len() - valid)..];
            }
        }
    }
    Ok(text)
}

/// The exec convenience ABI searches /bin for a bare name: the pathname
/// `path` names.
fn convenience_path(path: Vec<u8>) -> Result<Vec<u8>, u64> {
    if path.starts_with(b"/") {
        return Ok(path);
    }
    let base = if path.contains(&b'/') {
        crate::fs::namei::working_dir_path_bytes(&crate::fs::namei::current_working_dir())?
    } else {
        let mut base = Vec::new();
        base.try_reserve_exact(4).map_err(|_| ENOMEM as u64)?;
        base.extend_from_slice(b"/bin");
        base
    };
    let mut resolved = Vec::new();
    resolved.try_reserve_exact(base.len() + 1 + path.len()).map_err(|_| ENOMEM as u64)?;
    resolved.extend_from_slice(&base);
    if !base.ends_with(b"/") { resolved.push(b'/'); }
    resolved.extend_from_slice(&path);
    Ok(resolved)
}

/// Resolve scripts using byte pathnames and preserve every argument byte.
pub(crate) fn read_image<I>(
    path: impl AsRef<[u8]>,
    args: &mut Arguments,
    mut read: impl FnMut(&[u8]) -> Result<(Vec<u8>, I), i32>,
) -> Result<(Vec<u8>, I), u64> {
    let input = path.as_ref();
    let mut path = Vec::new();
    path.try_reserve_exact(input.len()).map_err(|_| ENOMEM as u64)?;
    path.extend_from_slice(input);
    for depth in 0..=5 {
        path = convenience_path(path)?;
        let (data, identity) = read(&path).map_err(|errno| if errno == EISDIR { EACCES as u64 } else { errno as u64 })?;
        if !data.starts_with(b"#!") {
            validate_elf(&data)?;
            return Ok((data, identity));
        }
        if depth == 5 { return Err(ELOOP as u64); }
        let end = data.iter().position(|&b| b == b'\n').unwrap_or(data.len());
        let mut line = &data[2..end];
        while line.first().is_some_and(|b| matches!(b, b' ' | b'\t')) { line = &line[1..]; }
        while line.last().is_some_and(|b| matches!(b, b' ' | b'\t')) { line = &line[..line.len()-1]; }
        let split = line.iter().position(|b| matches!(b, b' ' | b'\t')).unwrap_or(line.len());
        let interpreter = &line[..split];
        if interpreter.is_empty() { return Err(ENOEXEC as u64); }
        let mut optional = &line[split..];
        while optional.first().is_some_and(|b| matches!(b, b' ' | b'\t')) { optional = &optional[1..]; }
        args.interpreter(interpreter, optional, &path)?;
        path.clear();
        path.try_reserve(interpreter.len()).map_err(|_| ENOMEM as u64)?;
        path.extend_from_slice(interpreter);
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
