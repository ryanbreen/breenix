//! Bounded ELF64 reads for filesystem-backed process images.
//!
//! Exec, spawn and the boot images read the ELF header and program header
//! table into a small kernel buffer, then read each PT_LOAD segment's file
//! bytes straight into the frames mapped for the new image, at most CHUNK
//! bytes per read. Kernel heap use is the header table, independent of the
//! file size.
// The two standalone host harnesses exercise different parts of this module.
#![cfg_attr(test, allow(dead_code))]

use alloc::vec::Vec;
use core::{mem::size_of, ops::Range};
use crate::fs::errno::Errno;
use crate::syscall::errno::{ENOEXEC, ENOMEM};

pub(crate) const CHUNK: usize = 64 * 1024;
pub(crate) const ELF_HEADER: usize = 64;
const PHDR: usize = 56;
const PAGE: usize = 4096;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
// The pathname resolver accepts fewer than 4096 bytes, excluding the NUL.
const INTERP_PATH_MAX: u64 = 4096;

#[derive(Clone, Copy)]
struct ProgramHeader {
    kind: u32,
    flags: u32,
    offset: u64,
    address: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

impl ProgramHeader {
    fn parse(bytes: &[u8]) -> Self {
        let u32_at = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        Self {
            kind: u32_at(0), flags: u32_at(4), offset: u64_at(8),
            address: u64_at(16), filesz: u64_at(32), memsz: u64_at(40), align: u64_at(48),
        }
    }
}

type ReadAt<'a> = dyn FnMut(u64, &mut [u8]) -> Result<(), Errno> + 'a;
#[cfg(any(test, target_arch = "x86_64"))]
pub(crate) type ReadInterpreter<'a> = dyn FnMut(u64, &mut ReadAt<'_>) -> Result<ExecPlan, Errno> + 'a;

/// A page-aligned part of an interpreted exec image and its mapped ELF flags.
#[cfg(any(test, target_arch = "x86_64"))]
pub(crate) struct ImageRegion {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) flags: u32,
}

/// A validated ELF64 executable's header facts and program header table.
pub(crate) struct Headers {
    kind: u16,
    entry: u64,
    table: Vec<u8>,
    interpreter_path: Option<Vec<u8>>,
    phdr_vaddr: u64,
    first_page: u64,
    segments_end: u64,
    load_align: u64,
    program_bias: Option<u64>,
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
        let kind = u16_at(16);
        if (kind != ET_EXEC && !(kind == ET_DYN && machine == EM_X86_64))
            || u16_at(18) != machine
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
        let mut interpreter_path = None;
        let mut first_page = u64::MAX;
        let mut max_end = 0;
        let mut load_align = PAGE as u64;
        let mut image_size = 0u64;
        let mut data_size = 0u64;
        let mut explicit_phdr = None;
        let mut mapped_phdr = None;
        #[cfg(all(not(test), target_arch = "aarch64"))]
        let mut load_base = u64::MAX;
        for ph in table.chunks_exact(PHDR).map(ProgramHeader::parse) {
            match ph.kind {
                // gABI ch. 5: PT_INTERP is unique and precedes the loads.
                PT_INTERP => {
                    if machine != EM_X86_64 || interpreter_path.is_some() || first_page != u64::MAX {
                        return Err(Errno(ENOEXEC));
                    }
                    interpreter_path = Some(read_interpreter_path(ph, file_len, read)?);
                    continue;
                }
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
            first_page = first_page.min(ph.address & !(PAGE as u64 - 1));
            if kind == ET_DYN {
                if ph.offset % PAGE as u64 != ph.address % PAGE as u64
                    || (ph.align > 1 && (!ph.align.is_power_of_two()
                        || ph.offset % ph.align != ph.address % ph.align))
                {
                    return Err(Errno(ENOEXEC));
                }
                load_align = load_align.max(ph.align);
            }
            if ph.flags & 1 != 0 && (ph.address..memory_end).contains(&entry) {
                executable_entry = true;
            }
            if phoff >= ph.offset && table_end <= file_end {
                let address = ph.address + (phoff - ph.offset);
                // The x86 loader picked the first containing load and the ARM
                // loader the last one. Normal layouts have just one.
                if machine == EM_X86_64 {
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
        if !executable_entry
            || (interpreter_path.is_some() && (mapped_phdr.is_none()
                || (explicit_phdr.is_some() && explicit_phdr != mapped_phdr)))
        {
            return Err(Errno(ENOEXEC));
        }
        Ok(Self {
            kind,
            entry,
            table,
            interpreter_path,
            phdr_vaddr: explicit_phdr.or(mapped_phdr).unwrap_or(0),
            first_page,
            segments_end: (max_end + PAGE as u64 - 1) & !(PAGE as u64 - 1),
            load_align,
            program_bias: None,
            image_size,
            data_size,
            #[cfg(all(not(test), target_arch = "aarch64"))]
            load_base,
        })
    }

    /// The validated PT_INTERP pathname, without the trailing NUL.
    pub(crate) fn interpreter_path(&self) -> Option<&[u8]> {
        self.interpreter_path.as_deref()
    }

    /// Plan the handoff. For an interpreted program, the caller supplies the
    /// ET_DYN interpreter headers and the address range for its mappings.
    pub(crate) fn exec_plan(
        &self,
        interpreter: Option<(&Self, Range<u64>)>,
    ) -> Result<ExecPlan, Errno> {
        let bad = || Errno(ENOEXEC);
        let mut plan = self.program_plan()?;
        match (self.interpreter_path(), interpreter) {
            (None, None) => {}
            (Some(_), Some((image, region))) if image.kind == ET_DYN && image.interpreter_path().is_none() => {
                // Place the interpreter above the program's last load.
                // Align the bias itself to preserve each PT_LOAD's alignment.
                let start = region.start.max(self.segments_end());
                let bias = image.bias_in(start..region.end)?;
                plan.interpreter_bias = Some(bias);
                plan.initial_pc = image.entry + bias;
            }
            _ => return Err(bad()),
        }
        Ok(plan)
    }

    fn program_plan(&self) -> Result<ExecPlan, Errno> {
        if self.kind != ET_EXEC && self.program_bias.is_none() {
            return Err(Errno(ENOEXEC));
        }
        let bias = self.program_bias.unwrap_or(0);
        Ok(ExecPlan::static_image(
            self.phdr_vaddr + bias, (self.table.len() / PHDR) as u16, PHDR as u16, self.entry + bias,
        ))
    }

    fn bias_in(&self, region: Range<u64>) -> Result<u64, Errno> {
        let bad = || Errno(ENOEXEC);
        let mask = self.load_align - 1;
        let bias = region.start.saturating_sub(self.first_page)
            .checked_add(mask).ok_or_else(bad)? & !mask;
        bias.checked_add(self.segments_end)
            .filter(|&end| end <= region.end)
            .ok_or_else(bad)?;
        Ok(bias)
    }

    #[cfg(any(test, target_arch = "x86_64"))]
    pub(crate) fn position_program(mut self, region: Range<u64>) -> Result<Self, Errno> {
        if self.kind == ET_DYN && self.interpreter_path().is_some() {
            self.program_bias = Some(self.bias_in(region)?);
        }
        Ok(self)
    }

    fn segments_end(&self) -> u64 {
        self.segments_end + self.program_bias.unwrap_or(0)
    }

    /// Collect the page rights that map_image_page installed. If PT_LOADs
    /// share pages, those pages have the union of the segments' permissions.
    #[cfg(any(test, target_arch = "x86_64"))]
    fn append_image_regions(&self, bias: u64, regions: &mut Vec<ImageRegion>) -> Result<(), Errno> {
        let page_mask = PAGE as u64 - 1;
        let count = self.table.len() / PHDR;
        let mut loads = Vec::new();
        let mut boundaries = Vec::new();
        loads.try_reserve_exact(count).map_err(|_| Errno(ENOMEM))?;
        boundaries.try_reserve_exact(2 * count).map_err(|_| Errno(ENOMEM))?;
        for ph in self.table.chunks_exact(PHDR).map(ProgramHeader::parse) {
            if ph.kind != PT_LOAD || ph.memsz == 0 {
                continue;
            }
            let start = (ph.address + bias) & !page_mask;
            let end = (ph.address + bias + ph.memsz + page_mask) & !page_mask;
            loads.push((start..end, ph.flags));
            boundaries.extend([start, end]);
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        regions.try_reserve(boundaries.len()).map_err(|_| Errno(ENOMEM))?;
        for pair in boundaries.windows(2) {
            let [start, end] = [pair[0], pair[1]];
            let Some(flags) = loads.iter()
                .filter_map(|(range, flags)| range.contains(&start).then_some(*flags))
                .reduce(|combined, flags| combined | flags) else {
                continue;
            };
            if let Some(previous) = regions.last_mut()
                .filter(|previous| previous.end == start && previous.flags == flags)
            {
                previous.end = end;
            } else {
                regions.push(ImageRegion { start, end, flags });
            }
        }
        Ok(())
    }

    /// Put the named interpreter into the program's unpublished table. The
    /// table is dropped if opening, validating or reading the interpreter fails.
    ///
    /// # Safety
    /// `map_page` must return a writable page owned by `table`, as required
    /// by `load_segments`. `open` must retain the interpreter's file lease
    /// across the callback.
    #[cfg(any(test, target_arch = "x86_64"))]
    pub(crate) unsafe fn load_interpreter<T>(
        &self,
        mut table: T,
        region: Range<u64>,
        open: impl FnOnce(&[u8], &mut ReadInterpreter<'_>) -> Result<ExecPlan, Errno>,
        mut map_page: impl FnMut(&mut T, u64, u32) -> Result<*mut u8, Errno>,
    ) -> Result<(T, ExecPlan, Vec<ImageRegion>), Errno> {
        let path = self.interpreter_path().ok_or(Errno(ENOEXEC))?;
        let mut regions = Vec::new();
        let plan = open(path, &mut |size, read| {
            let mut header = [0; ELF_HEADER];
            let prefix = size.min(ELF_HEADER as u64) as usize;
            read(0, &mut header[..prefix])?;
            let interpreter = Self::read(&header[..prefix], size, EM_X86_64, region.end, read)?;
            let plan = self.exec_plan(Some((&interpreter, region.clone())))?;
            let bias = plan.interpreter_bias().ok_or(Errno(ENOEXEC))?;
            self.append_image_regions(self.program_bias.unwrap_or(0), &mut regions)?;
            interpreter.append_image_regions(bias, &mut regions)?;
            // SAFETY: map_page supplies the pages owned by table. exec_plan
            // bounds the biased loads and places them above the program.
            unsafe {
                interpreter.load_segments(
                    read,
                    |address, flags| map_page(&mut table, address + bias, flags),
                    |_, _| {},
                )?;
            }
            Ok(plan)
        })?;
        Ok((table, plan, regions))
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
        let bias = self.program_bias.unwrap_or(0);
        for ph in self.table.chunks_exact(PHDR).map(ProgramHeader::parse) {
            if ph.kind != PT_LOAD || ph.memsz == 0 {
                continue;
            }
            let begin = ph.address + bias;
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

fn read_interpreter_path(
    ph: ProgramHeader,
    file_len: u64,
    read: &mut ReadAt<'_>,
) -> Result<Vec<u8>, Errno> {
    if !(2..=INTERP_PATH_MAX).contains(&ph.filesz)
        || ph.offset.checked_add(ph.filesz).is_none_or(|end| end > file_len)
    {
        return Err(Errno(ENOEXEC));
    }
    let len = ph.filesz as usize;
    let mut path = Vec::new();
    path.try_reserve_exact(len).map_err(|_| Errno(ENOMEM))?;
    path.resize(len, 0);
    read(ph.offset, &mut path)?;
    if path.pop() != Some(0) || path.contains(&0) {
        return Err(Errno(ENOEXEC));
    }
    Ok(path)
}

/// The program metadata on the initial stack and the PC it starts at. Under
/// AMD64 psABI §3.4, the auxv still describes the program on interpreter entry.
#[derive(Clone, Copy)]
pub(crate) struct ExecPlan {
    phdr_vaddr: u64,
    phnum: u16,
    phentsize: u16,
    program_entry: u64,
    interpreter_bias: Option<u64>,
    initial_pc: u64,
}

impl ExecPlan {
    const PREFIX_PAIRS: usize = 5;
    const SUFFIX_PAIRS: usize = 7;
    const PAIR_SIZE: usize = size_of::<(u64, u64)>();
    pub(crate) const STATIC_AUXV_SIZE: usize =
        (Self::PREFIX_PAIRS + Self::SUFFIX_PAIRS) * Self::PAIR_SIZE;

    pub(crate) fn static_image(phdr_vaddr: u64, phnum: u16, phentsize: u16, entry: u64) -> Self {
        Self {
            phdr_vaddr,
            phnum,
            phentsize,
            program_entry: entry,
            interpreter_bias: None,
            initial_pc: entry,
        }
    }

    pub(crate) fn interpreter_bias(&self) -> Option<u64> {
        self.interpreter_bias
    }

    pub(crate) fn initial_pc(&self) -> u64 {
        self.initial_pc
    }

    pub(crate) fn auxv_size(&self) -> usize {
        Self::STATIC_AUXV_SIZE + usize::from(self.interpreter_bias().is_some()) * Self::PAIR_SIZE
    }

    pub(crate) fn auxv(
        &self,
        random_addr: u64,
        [uid, euid, gid, egid, secure]: [u64; 5],
    ) -> impl Iterator<Item = (u64, u64)> {
        let prefix: [_; Self::PREFIX_PAIRS] = [
            (9, self.program_entry),         // AT_ENTRY
            (3, self.phdr_vaddr),            // AT_PHDR
            (5, u64::from(self.phnum)),      // AT_PHNUM
            (4, u64::from(self.phentsize)),  // AT_PHENT
            (6, PAGE as u64),               // AT_PAGESZ
        ];
        let suffix: [_; Self::SUFFIX_PAIRS] = [
            (25, random_addr), // AT_RANDOM
            (11, uid),         // AT_UID
            (12, euid),        // AT_EUID
            (13, gid),         // AT_GID
            (14, egid),        // AT_EGID
            (23, secure),      // AT_SECURE
            (0, 0),            // AT_NULL
        ];
        prefix.into_iter()
            .chain(self.interpreter_bias().map(|bias| (7, bias))) // AT_BASE
            .chain(suffix)
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
    #[cfg(target_arch = "x86_64")]
    interpreter_plan: Option<ExecPlan>,
    #[cfg(target_arch = "x86_64")]
    exec_vmas: Vec<crate::memory::vma::Vma>,
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
    ) -> Result<(Self, Option<Headers>), Errno> {
        #[cfg(target_arch = "x86_64")]
        let machine = arch::EM_X86_64;
        #[cfg(target_arch = "aarch64")]
        let machine = arch::EM_AARCH64;
        let headers = Headers::read(header, file_len, machine,
            crate::memory::layout::USER_STACK_REGION_END, read)?;
        let needs_interpreter = purpose == Purpose::Exec && headers.interpreter_path().is_some();
        #[cfg(target_arch = "x86_64")]
        let headers = if needs_interpreter {
            use crate::memory::layout::{USERSPACE_BASE, USERSPACE_CODE_DATA_END};
            headers.position_program(USERSPACE_BASE..USERSPACE_CODE_DATA_END)?
        } else {
            headers
        };
        let plan = if needs_interpreter {
            headers.program_plan()?
        } else {
            headers.exec_plan(None)?
        };
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
            entry_point: x86_64::VirtAddr::new(plan.initial_pc()),
            #[cfg(target_arch = "aarch64")]
            entry_point: plan.initial_pc(),
            #[cfg(target_arch = "x86_64")]
            stack_top: x86_64::VirtAddr::zero(),
            #[cfg(target_arch = "aarch64")]
            load_base: headers.load_base,
            segments_end: headers.segments_end(),
            image_size: headers.image_size,
            data_size: headers.data_size,
            phdr_vaddr: plan.phdr_vaddr,
            phnum: plan.phnum,
            phentsize: plan.phentsize,
        };
        let image = Self {
            table,
            elf,
            #[cfg(target_arch = "x86_64")]
            interpreter_plan: None,
            #[cfg(target_arch = "x86_64")]
            exec_vmas: Vec::new(),
            file_len,
        };
        Ok((image, needs_interpreter.then_some(headers)))
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn with_interpreter(
        mut self,
        program: &Headers,
        open: impl FnOnce(&[u8], &mut ReadInterpreter<'_>) -> Result<ExecPlan, Errno>,
    ) -> Result<Self, Errno> {
        use crate::memory::layout::{MMAP_REGION_END, MMAP_REGION_START};
        use crate::memory::vma::{MmapFlags, Protection, Vma};
        // SAFETY: arch::map_image_page returns the table's own direct-map
        // pages, and the VFS opener pins the file through its reads.
        let (table, plan, regions) = unsafe {
            program.load_interpreter(
                self.table,
                MMAP_REGION_START..MMAP_REGION_END,
                open,
                |table, address, flags| arch::map_image_page(table.as_mut(), address, flags).map_err(Errno),
            )?
        };
        self.exec_vmas.try_reserve_exact(regions.len()).map_err(|_| Errno(ENOMEM))?;
        let flags = MmapFlags::from_bits_truncate(MmapFlags::PRIVATE.bits() | MmapFlags::ANONYMOUS.bits());
        for region in regions {
            // ELF PF_R and PF_X occupy the opposite bits from PROT_READ
            // and PROT_EXEC. PF_W and PROT_WRITE both occupy bit 1.
            let elf = region.flags;
            let prot = Protection::from_bits_truncate((elf & 4) >> 2 | (elf & 2) | (elf & 1) << 2);
            self.exec_vmas.push(Vma::new(
                x86_64::VirtAddr::new(region.start),
                x86_64::VirtAddr::new(region.end),
                prot,
                flags,
            ));
        }
        self.table = table;
        self.elf.entry_point = x86_64::VirtAddr::new(plan.initial_pc());
        // The VMAs now account for both images, including the writable bytes.
        self.elf.image_size = 0;
        self.elf.data_size = 0;
        self.interpreter_plan = Some(plan);
        Ok(self)
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
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn take_exec_vmas(&mut self) -> Vec<crate::memory::vma::Vma> {
        match self {
            Self::Mapped(image) => core::mem::take(&mut image.exec_vmas),
            Self::Bytes(_) | Self::Owned(_) => Vec::new(),
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn interpreter_plan(&self) -> Option<ExecPlan> {
        match self {
            Self::Mapped(image) => image.interpreter_plan,
            Self::Bytes(_) | Self::Owned(_) => None,
        }
    }

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
