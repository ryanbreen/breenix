//! Bounded ELF64 reads for filesystem-backed process images.
//!
//! Exec, spawn and the boot images read the ELF header and program header
//! table into a small kernel buffer, then read each PT_LOAD segment's file
//! bytes straight into the frames mapped for the new image, at most CHUNK
//! bytes per read. Kernel heap use is the header table, independent of the
//! file size.
use alloc::vec::Vec;
use crate::fs::errno::Errno;
use crate::syscall::errno::{ENOEXEC, ENOMEM};

pub(crate) const CHUNK: usize = 64 * 1024;
pub(crate) const ELF_HEADER: usize = 64;
const PHDR: usize = 56;
const PAGE: usize = 4096;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;

#[derive(Clone, Copy)]
struct ProgramHeader {
    kind: u32,
    flags: u32,
    offset: u64,
    address: u64,
    filesz: u64,
    memsz: u64,
}

impl ProgramHeader {
    fn parse(bytes: &[u8]) -> Self {
        let u32_at = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        Self {
            kind: u32_at(0), flags: u32_at(4), offset: u64_at(8),
            address: u64_at(16), filesz: u64_at(32), memsz: u64_at(40),
        }
    }
}

type ReadAt<'a> = dyn FnMut(u64, &mut [u8]) -> Result<(), Errno> + 'a;

/// A validated ELF64 executable's header facts and program header table.
pub(crate) struct Headers {
    entry: u64,
    table: Vec<u8>,
    phdr_vaddr: u64,
    segments_end: u64,
    image_size: u64,
    data_size: u64,
    #[cfg(all(not(test), target_arch = "aarch64"))]
    load_base: u64,
}

impl Headers {
    /// Check the executable with exec's former checks (ENOEXEC for what either
    /// loader cannot run) and read its program header table, which may be
    /// at most CHUNK bytes.
    pub(crate) fn read(
        header: &[u8],
        file_len: u64,
        machine: u16,
        user_end: u64,
        read: &mut ReadAt<'_>,
    ) -> Result<Self, Errno> {
        let bad = Errno(ENOEXEC);
        if header.len() != ELF_HEADER || &header[..7] != b"\x7fELF\x02\x01\x01" {
            return Err(bad);
        }
        let u16_at = |at| u16::from_le_bytes(header[at..at + 2].try_into().unwrap());
        let u64_at = |at| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
        if u16_at(16) != 2 || u16_at(18) != machine
            || u16_at(52) as usize != ELF_HEADER || u16_at(54) as usize != PHDR
        {
            return Err(bad);
        }
        let count = u16_at(56) as usize;
        let table_len = count * PHDR;
        if count == 0 || table_len > CHUNK {
            return Err(bad);
        }
        let phoff = u64_at(32);
        let table_end = phoff.checked_add(table_len as u64).ok_or(Errno(ENOEXEC))?;
        if table_end > file_len {
            return Err(Errno(ENOEXEC));
        }
        let mut table = Vec::new();
        table.try_reserve_exact(table_len).map_err(|_| Errno(ENOMEM))?;
        table.resize(table_len, 0);
        read(phoff, &mut table)?;

        let entry = u64_at(24);
        let mut executable_entry = false;
        let mut max_end = 0;
        let mut image_size = 0u64;
        let mut data_size = 0u64;
        let mut explicit_phdr = None;
        let mut mapped_phdr = None;
        #[cfg(all(not(test), target_arch = "aarch64"))]
        let mut load_base = u64::MAX;
        for ph in table.chunks_exact(PHDR).map(ProgramHeader::parse) {
            match ph.kind {
                PT_INTERP => return Err(Errno(ENOEXEC)),
                PT_PHDR if ph.address < user_end => {
                    explicit_phdr = Some(ph.address);
                    continue;
                }
                PT_PHDR => return Err(Errno(ENOEXEC)),
                PT_LOAD => {}
                _ => continue,
            }
            let file_end = ph.offset.checked_add(ph.filesz).ok_or(Errno(ENOEXEC))?;
            let memory_end = ph.address.checked_add(ph.memsz).ok_or(Errno(ENOEXEC))?;
            if ph.filesz > ph.memsz || memory_end > user_end || file_end > file_len {
                return Err(Errno(ENOEXEC));
            }
            if ph.flags & 1 != 0 && (ph.address..memory_end).contains(&entry) {
                executable_entry = true;
            }
            if phoff >= ph.offset && table_end <= file_end {
                let address = ph.address + (phoff - ph.offset);
                // The x86 loader picked the first containing load and the ARM
                // loader the last one. Normal layouts have just one.
                if machine == 62 {
                    mapped_phdr = mapped_phdr.or(Some(address));
                } else {
                    mapped_phdr = Some(address);
                }
            }
            let bytes = (((ph.address % PAGE as u64) + ph.memsz + PAGE as u64 - 1)
                / PAGE as u64) * PAGE as u64;
            image_size = image_size.checked_add(bytes).ok_or(Errno(ENOEXEC))?;
            if ph.flags & 2 != 0 {
                data_size = data_size.checked_add(bytes).ok_or(Errno(ENOEXEC))?;
            }
            max_end = max_end.max(memory_end);
            #[cfg(all(not(test), target_arch = "aarch64"))]
            { load_base = load_base.min(ph.address); }
        }
        if !executable_entry {
            return Err(Errno(ENOEXEC));
        }
        Ok(Self {
            entry,
            table,
            phdr_vaddr: explicit_phdr.or(mapped_phdr).unwrap_or(0),
            segments_end: (max_end + PAGE as u64 - 1) & !(PAGE as u64 - 1),
            image_size,
            data_size,
            #[cfg(all(not(test), target_arch = "aarch64"))]
            load_base,
        })
    }

    /// The entry point, program header address, heap start, image size and
    /// writable size the process is built with.
    #[cfg(test)]
    pub(crate) fn summary(&self) -> [u64; 5] {
        [self.entry, self.phdr_vaddr, self.segments_end, self.image_size, self.data_size]
    }

    /// Map pages in PT_LOAD order and read each consecutive physical run into
    /// those pages. A discontinuity or CHUNK bytes ends a read window.
    ///
    /// # Safety
    /// `map_page` must return a writable page-sized range that this image
    /// owns: a fresh zero-filled page, or a page an earlier segment of this
    /// same image mapped.
    pub(crate) unsafe fn load_segments(
        &self,
        read: &mut ReadAt<'_>,
        mut map_page: impl FnMut(u64, u32) -> Result<*mut u8, Errno>,
        mut sync: impl FnMut(*mut u8, usize),
    ) -> Result<(), Errno> {
        for ph in self.table.chunks_exact(PHDR).map(ProgramHeader::parse) {
            if ph.kind != PT_LOAD || ph.memsz == 0 {
                continue;
            }
            let begin = ph.address;
            let file_end = begin + ph.filesz;
            let memory_end = begin + ph.memsz;
            let first = begin & !(PAGE as u64 - 1);
            let mut run = FrameRun { start: core::ptr::null_mut(), offset: ph.offset, len: 0 };
            for address in (first..memory_end).step_by(PAGE) {
                let ptr = map_page(address, ph.flags)?;
                let start = address.max(begin);
                if start < file_end {
                    let count = (address + PAGE as u64).min(file_end) - start;
                    // SAFETY: map_page provides a writable page, and this
                    // subrange lies within it.
                    let dst = unsafe { ptr.add((start - address) as usize) };
                    if run.len != 0 &&
                        (run.len + count as usize > CHUNK || run.start.wrapping_add(run.len) != dst)
                    {
                        // SAFETY: the run's ranges are adjacent writable parts
                        // of pages belonging to this image.
                        unsafe { run.flush(read, &mut sync)?; }
                    }
                    if run.len == 0 {
                        run.start = dst;
                        run.offset = ph.offset + (start - begin);
                    }
                    run.len += count as usize;
                } else if run.len != 0 {
                    // SAFETY: the run contains only this image's pages.
                    unsafe { run.flush(read, &mut sync)?; }
                }
            }
            if run.len != 0 {
                // SAFETY: the run contains only this image's pages.
                unsafe { run.flush(read, &mut sync)?; }
            }
        }
        Ok(())
    }
}

struct FrameRun {
    start: *mut u8,
    offset: u64,
    len: usize,
}

impl FrameRun {
    /// # Safety
    /// `start..start + len` is writable memory this image owns.
    unsafe fn flush(
        &mut self,
        read: &mut ReadAt<'_>,
        sync: &mut impl FnMut(*mut u8, usize),
    ) -> Result<(), Errno> {
        // SAFETY: the caller supplies this range under the contract above.
        let bytes = unsafe { core::slice::from_raw_parts_mut(self.start, self.len) };
        read(self.offset, bytes)?;
        sync(self.start, self.len);
        self.len = 0;
        Ok(())
    }
}

#[cfg(not(test))]
use crate::memory::process_memory::{ProcessPageTable, UnpublishedPageTable};

#[cfg(all(not(test), target_arch = "x86_64"))]
use crate::elf as arch;
#[cfg(all(not(test), target_arch = "aarch64"))]
use crate::arch_impl::aarch64::elf as arch;

/// How the new image's page table starts out.
#[cfg(not(test))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A replacement image: `#!` scripts are allowed, and the table is
    /// cleared by `ProcessManager::clear_exec_table`, as exec's own table is.
    Exec,
    /// A new process (spawn, init, boot programs): an ELF only, in a fresh table.
    Spawn,
}

/// An ELF and its never-installed page table, ready for stack setup and
/// publication. Neither the VNode lease nor the mapping gate escape the read.
#[cfg(not(test))]
pub struct MappedElf {
    table: UnpublishedPageTable,
    elf: arch::LoadedElf,
    /// The file's length, reported as the process's code size.
    pub file_len: u64,
}

#[cfg(not(test))]
impl MappedElf {
    pub(crate) fn load(
        header: &[u8],
        file_len: u64,
        read: &mut ReadAt<'_>,
        purpose: Purpose,
    ) -> Result<Self, Errno> {
        #[cfg(target_arch = "x86_64")]
        let machine = arch::EM_X86_64;
        #[cfg(target_arch = "aarch64")]
        let machine = arch::EM_AARCH64;
        let headers = Headers::read(header, file_len, machine,
            crate::memory::layout::USER_STACK_REGION_END, read)?;
        // Reserve the Box before allocating a root, so ENOMEM cannot strand it.
        let mut table = alloc::boxed::Box::try_new(core::mem::MaybeUninit::<ProcessPageTable>::uninit())
            .map_err(|_| Errno(ENOMEM))?;
        table.write(ProcessPageTable::new().map_err(|_| Errno(ENOMEM))?);
        // SAFETY: the write above initialized the boxed table, which has the same layout.
        let table = unsafe { table.assume_init() };
        let mut table = UnpublishedPageTable::from_box(table, 0);
        if purpose == Purpose::Exec {
            crate::process::manager::ProcessManager::clear_exec_table(table.as_mut());
        }
        // SAFETY: arch::map_image_page yields direct-map pages owned by this
        // unpublished table, which lives until this function returns.
        unsafe {
            headers.load_segments(
                read,
                |address, flags| arch::map_image_page(table.as_mut(), address, flags).map_err(Errno),
                arch::sync_image_range,
            )?;
        }
        arch::finish_image_load();
        let elf = arch::LoadedElf {
            #[cfg(target_arch = "x86_64")]
            entry_point: x86_64::VirtAddr::new(headers.entry),
            #[cfg(target_arch = "aarch64")]
            entry_point: headers.entry,
            #[cfg(target_arch = "x86_64")]
            stack_top: x86_64::VirtAddr::zero(),
            #[cfg(target_arch = "aarch64")]
            load_base: headers.load_base,
            segments_end: headers.segments_end,
            image_size: headers.image_size,
            data_size: headers.data_size,
            phdr_vaddr: headers.phdr_vaddr,
            phnum: (headers.table.len() / PHDR) as u16,
            phentsize: PHDR as u16,
        };
        Ok(Self { table, elf, file_len })
    }
}

/// What exec found at a pathname: an ELF already mapped, or a `#!` line.
#[cfg(not(test))]
pub enum Image {
    Elf(MappedElf),
    /// The interpreter line after `#!`, without its surrounding blanks.
    Script(Vec<u8>),
    /// A whole program already in memory, from outside the filesystem (the
    /// x86 test disk).
    Bytes(Vec<u8>),
}

#[cfg(not(test))]
impl Image {
    pub fn into_elf(self) -> Result<MappedElf, Errno> {
        match self {
            Self::Elf(image) => Ok(image),
            Self::Script(_) | Self::Bytes(_) => Err(Errno(ENOEXEC)),
        }
    }
}

/// Read the interpreter line, discarding the script body, and bound the line
/// by the exec argument budget. Blanks before and after the line do not
/// consume that budget.
#[cfg(not(test))]
pub(crate) fn read_shebang(prefix: &[u8], size: u64, read: &mut ReadAt<'_>) -> Result<Vec<u8>, Errno> {
    use crate::syscall::errno::E2BIG;
    let mut line = Vec::new();
    let mut significant = 0usize;
    let mut whitespace = 0usize;
    let mut limit_exceeded = false;
    let mut append = |bytes: &[u8]| -> Result<bool, Errno> {
        for &byte in bytes {
            if byte == b'\n' {
                return Ok(true);
            }
            if matches!(byte, b' ' | b'\t') {
                if significant != 0 {
                    whitespace = whitespace.saturating_add(1);
                }
            } else {
                if limit_exceeded || significant.saturating_add(whitespace) >= crate::syscall::exec::ARG_MAX {
                    return Err(Errno(E2BIG));
                }
                significant += whitespace + 1;
                whitespace = 0;
            }
            if significant != 0 && !limit_exceeded {
                if line.len() == crate::syscall::exec::ARG_MAX {
                    limit_exceeded = true;
                } else {
                    line.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
                    line.push(byte);
                }
            }
        }
        Ok(false)
    };

    if !append(prefix)? {
        let mut offset = 2 + prefix.len() as u64;
        let mut scratch = [0; 1024];
        while offset < size {
            let count = (size - offset).min(scratch.len() as u64) as usize;
            read(offset, &mut scratch[..count])?;
            if append(&scratch[..count])? { break; }
            offset += count as u64;
        }
    }
    line.truncate(significant);
    Ok(line)
}

/// The image a process constructor loads: bytes already in memory (test and
/// built-in programs), or a filesystem ELF already read into its own table,
/// so the process-manager guard taken by the constructor spans no filesystem I/O.
#[cfg(not(test))]
pub enum ProcessImage<'a> {
    Bytes(&'a [u8]),
    Owned(Vec<u8>),
    Mapped(MappedElf),
}

#[cfg(not(test))]
impl<'a> ProcessImage<'a> {
    /// The image's file length, for ARM's process-creation diagnostics.
    #[cfg(target_arch = "aarch64")]
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Owned(bytes) => bytes.len(),
            Self::Mapped(image) => image.file_len as usize,
        }
    }

    /// A mapped image's own table (re-keyed to `pid`), loaded ELF and code
    /// size, or the in-memory bytes the constructor loads into its own table.
    pub(crate) fn into_loaded(self, pid: u64) -> Loaded<'a> {
        match self {
            Self::Bytes(bytes) => Loaded::Bytes(alloc::borrow::Cow::Borrowed(bytes)),
            Self::Owned(bytes) => Loaded::Bytes(alloc::borrow::Cow::Owned(bytes)),
            Self::Mapped(image) => Loaded::Mapped(
                UnpublishedPageTable::from_box(image.table.publish(), pid),
                image.elf,
                image.file_len as usize,
            ),
        }
    }
}

/// What a process constructor starts from: see `ProcessImage::into_loaded`.
#[cfg(not(test))]
pub(crate) enum Loaded<'a> {
    Mapped(UnpublishedPageTable, arch::LoadedElf, usize),
    Bytes(alloc::borrow::Cow<'a, [u8]>),
}

#[cfg(not(test))]
impl<'a> From<&'a [u8]> for ProcessImage<'a> {
    fn from(bytes: &'a [u8]) -> Self { Self::Bytes(bytes) }
}

#[cfg(not(test))]
impl<'a, const N: usize> From<&'a [u8; N]> for ProcessImage<'a> {
    fn from(bytes: &'a [u8; N]) -> Self { Self::Bytes(bytes) }
}

#[cfg(not(test))]
impl<'a> From<&'a Vec<u8>> for ProcessImage<'a> {
    fn from(bytes: &'a Vec<u8>) -> Self { Self::Bytes(bytes) }
}

#[cfg(not(test))]
impl From<MappedElf> for ProcessImage<'_> {
    fn from(image: MappedElf) -> Self { Self::Mapped(image) }
}
