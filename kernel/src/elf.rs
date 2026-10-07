//! ELF64 loader for executing userspace programs
//!
//! Note: This module is x86_64-only. ARM64 has its own ELF loader in arch_impl/aarch64/elf.rs

#![cfg(target_arch = "x86_64")]

use core::mem;
use x86_64::{
    structures::paging::{Mapper, Page, PageTableFlags, Size4KiB},
    VirtAddr,
};

/// ELF magic number
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF class (64-bit)
pub const ELFCLASS64: u8 = 2;

/// ELF data encoding (little-endian)
pub const ELFDATA2LSB: u8 = 1;

/// ELF machine type (x86-64)
pub const EM_X86_64: u16 = 62;

/// ELF file header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    pub magic: [u8; 4],
    pub class: u8,
    pub data: u8,
    pub version: u8,
    pub osabi: u8,
    pub abiversion: u8,
    pub _pad: [u8; 7],
    pub elf_type: u16,
    pub machine: u16,
    pub version2: u32,
    pub entry: u64,
    pub phoff: u64,
    pub shoff: u64,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

/// Program header types
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegmentType {
    #[allow(dead_code)]
    Null = 0,
    Load = 1,
    #[allow(dead_code)]
    Dynamic = 2,
    #[allow(dead_code)]
    Interp = 3,
    #[allow(dead_code)]
    Note = 4,
    #[allow(dead_code)]
    Shlib = 5,
    #[allow(dead_code)]
    Phdr = 6,
    #[allow(dead_code)]
    Tls = 7,
}

/// Program header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

/// ELF loader result
pub struct LoadedElf {
    pub entry_point: VirtAddr,
    #[allow(dead_code)]
    pub stack_top: VirtAddr,
    /// End of loaded segments, page-aligned up (start of heap)
    pub segments_end: u64,
    pub image_size: u64,
    pub data_size: u64,
    /// Virtual address of program headers in their mapped PT_LOAD segment
    pub phdr_vaddr: u64,
    /// Number of program headers
    pub phnum: u16,
    /// Size of each program header entry
    pub phentsize: u16,
}

// Reject malformed user addresses before constructing VirtAddr or mapping pages.
fn user_address(address: u64) -> Result<VirtAddr, &'static str> {
    let address = VirtAddr::try_new(address).map_err(|_| "Non-canonical ELF address")?;
    if address.as_u64() >= crate::memory::layout::USER_STACK_REGION_END {
        return Err("ELF address outside userspace");
    }
    Ok(address)
}

fn biased_address(address: u64, base: VirtAddr) -> Result<VirtAddr, &'static str> {
    let address = if address >= crate::memory::layout::USERSPACE_BASE {
        address
    } else {
        base.as_u64()
            .checked_add(address)
            .ok_or("ELF address overflow")?
    };
    user_address(address)
}

fn program_header_offset(
    header: &Elf64Header,
    index: usize,
    len: usize,
) -> Result<usize, &'static str> {
    if header.phentsize as usize != mem::size_of::<Elf64ProgramHeader>() {
        return Err("Invalid program header size");
    }
    let start = (header.phoff as usize)
        .checked_add(
            index
                .checked_mul(header.phentsize as usize)
                .ok_or("Program header overflow")?,
        )
        .ok_or("Program header overflow")?;
    if start
        .checked_add(mem::size_of::<Elf64ProgramHeader>())
        .ok_or("Program header overflow")?
        > len
    {
        return Err("Program header out of bounds");
    }
    Ok(start)
}

fn mapped_program_headers(
    header: &Elf64Header,
    ph: &Elf64ProgramHeader,
    vaddr: u64,
) -> Result<Option<u64>, &'static str> {
    let size = (header.phnum as u64)
        .checked_mul(header.phentsize as u64)
        .ok_or("Program header overflow")?;
    let end = header
        .phoff
        .checked_add(size)
        .ok_or("Program header overflow")?;
    let file_end = ph
        .p_offset
        .checked_add(ph.p_filesz)
        .ok_or("Segment file range overflow")?;
    if header.phoff >= ph.p_offset && end <= file_end {
        let address = vaddr
            .checked_add(header.phoff - ph.p_offset)
            .ok_or("Program header address overflow")?;
        user_address(address)?;
        return Ok(Some(address));
    }
    Ok(None)
}

/// Load an ELF64 binary into memory
pub fn load_elf(data: &[u8]) -> Result<LoadedElf, &'static str> {
    load_elf_at_base(data, VirtAddr::zero())
}

/// Load an ELF64 binary into memory with a base address offset
pub fn load_elf_at_base(data: &[u8], base_offset: VirtAddr) -> Result<LoadedElf, &'static str> {
    log::trace!(
        "load_elf_at_base: data size = {} bytes, base = {:#x}",
        data.len(),
        base_offset.as_u64()
    );

    // Verify ELF header
    if data.len() < mem::size_of::<Elf64Header>() {
        log::error!(
            "ELF file too small: {} < {}",
            data.len(),
            mem::size_of::<Elf64Header>()
        );
        return Err("ELF file too small");
    }

    // Copy header data to avoid alignment issues
    let mut header_bytes = [0u8; mem::size_of::<Elf64Header>()];
    header_bytes.copy_from_slice(&data[..mem::size_of::<Elf64Header>()]);
    let header: &Elf64Header = unsafe { &*(header_bytes.as_ptr() as *const Elf64Header) };

    log::trace!("ELF header loaded");

    // Verify magic number
    if header.magic != ELF_MAGIC {
        log::error!("Invalid ELF magic: {:?} != {:?}", header.magic, ELF_MAGIC);
        return Err("Invalid ELF magic");
    }

    // Verify 64-bit ELF
    if header.class != ELFCLASS64 {
        log::error!("Not a 64-bit ELF: class = {}", header.class);
        return Err("Not a 64-bit ELF");
    }

    // Verify little-endian
    if header.data != ELFDATA2LSB {
        log::error!("Not little-endian ELF: data = {}", header.data);
        return Err("Not little-endian ELF");
    }

    // Verify executable
    if header.elf_type != 2 {
        log::error!("Not an executable ELF: type = {}", header.elf_type);
        return Err("Not an executable ELF");
    }

    // Verify x86_64
    if header.machine != 0x3e {
        log::error!("Not an x86_64 ELF: machine = {:#x}", header.machine);
        return Err("Not an x86_64 ELF");
    }

    log::info!(
        "Loading ELF: entry={:#x}, {} program headers",
        header.entry,
        header.phnum
    );

    // Process program headers
    let ph_count = header.phnum as usize;

    // Track the maximum end of all loaded segments for heap start calculation
    let mut image_size = 0u64;
    let mut data_size = 0u64;
    let mut max_segment_end = 0u64;
    // Track PT_PHDR for auxv
    let mut phdr_vaddr: Option<u64> = None;
    let mut mapped_phdr_vaddr: Option<u64> = None;

    for i in 0..ph_count {
        let ph_start = program_header_offset(header, i, data.len())?;

        // Copy program header to avoid alignment issues
        let mut ph_bytes = [0u8; mem::size_of::<Elf64ProgramHeader>()];
        ph_bytes.copy_from_slice(&data[ph_start..ph_start + mem::size_of::<Elf64ProgramHeader>()]);
        let ph: &Elf64ProgramHeader = unsafe { &*(ph_bytes.as_ptr() as *const Elf64ProgramHeader) };

        // Check for PT_PHDR segment
        if ph.p_type == SegmentType::Phdr as u32 {
            phdr_vaddr = Some(biased_address(ph.p_vaddr, base_offset)?.as_u64());
        }

        if ph.p_type == SegmentType::Load as u32 {
            let bytes = (ph.p_vaddr & 4095)
                .checked_add(ph.p_memsz)
                .and_then(|n| n.checked_add(4095))
                .ok_or("Segment size overflow")?
                & !4095;
            image_size = image_size.checked_add(bytes).ok_or("Image size overflow")?;
            if ph.p_flags & 2 != 0 {
                data_size = data_size.checked_add(bytes).ok_or("Data size overflow")?;
            }
            load_segment(data, ph, base_offset)?;

            // Calculate end of this segment (vaddr + memsz) considering base offset
            let vaddr = biased_address(ph.p_vaddr, base_offset)?.as_u64();
            mapped_phdr_vaddr = mapped_phdr_vaddr.or(mapped_program_headers(header, ph, vaddr)?);
            let segment_end = vaddr
                .checked_add(ph.p_memsz)
                .ok_or("Segment address overflow")?;
            if segment_end > max_segment_end {
                max_segment_end = segment_end;
            }
        }
    }

    // Align heap start to next page boundary (4KB)
    let heap_start = max_segment_end
        .checked_add(0xfff)
        .ok_or("Heap address overflow")?
        & !0xfff;

    // AT_PHDR is a mapped address, not a file offset. Match the PT_LOAD
    // containing e_phoff, including that segment's load bias and file offset.
    let phdr_vaddr = phdr_vaddr.or(mapped_phdr_vaddr).unwrap_or(0);

    // The entry point should be the header entry point directly
    // since our userspace binaries are compiled with absolute addresses
    Ok(LoadedElf {
        entry_point: biased_address(header.entry, base_offset)?,
        stack_top: VirtAddr::zero(), // Stack will be allocated by spawn function
        segments_end: heap_start,
        image_size,
        data_size,
        phdr_vaddr,
        phnum: header.phnum,
        phentsize: header.phentsize,
    })
}

/// Load a program segment into memory
fn load_segment(
    data: &[u8],
    ph: &Elf64ProgramHeader,
    base_offset: VirtAddr,
) -> Result<(), &'static str> {
    // Validate segment
    let file_start = ph.p_offset as usize;
    let file_size = ph.p_filesz as usize;
    let mem_size = ph.p_memsz as usize;

    if file_size > mem_size {
        return Err("Segment file size exceeds memory size");
    }
    if mem_size == 0 {
        return Ok(());
    }

    // Our userspace binaries use absolute addressing starting at USERSPACE_BASE
    // Don't add base_offset for absolute addresses in the userspace range
    let vaddr = biased_address(ph.p_vaddr, base_offset)?;

    if file_start
        .checked_add(file_size)
        .ok_or("Segment file range overflow")?
        > data.len()
    {
        return Err("Segment data out of bounds");
    }

    log::trace!(
        "Loading segment: vaddr={:#x}, filesz={:#x}, memsz={:#x}, flags={:#x}",
        vaddr.as_u64(),
        file_size,
        mem_size,
        ph.p_flags
    );

    // Calculate pages needed
    let end_addr = user_address(
        vaddr
            .as_u64()
            .checked_add(mem_size as u64 - 1)
            .ok_or("Segment address overflow")?,
    )?;
    // Iterate integer addresses: PageRangeInclusive advances past its last
    // page, which panics at the end of the canonical user address range.
    let page_addresses = vaddr.align_down(4096u64).as_u64()..=end_addr.as_u64();

    // Map pages
    let mut mapper = unsafe { crate::memory::paging::get_mapper() };

    // Initially map all pages as writable so we can load the data
    // We'll fix permissions later if needed
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;

    let segment_writable = ph.p_flags & 2 != 0;
    let segment_executable = ph.p_flags & 1 != 0;

    log::trace!(
        "Segment permissions: readable={}, writable={}, executable={}",
        ph.p_flags & 4 != 0,
        segment_writable,
        segment_executable
    );

    // Map all pages for the segment
    for address in page_addresses.clone().step_by(4096) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
        log::trace!(
            "Allocating frame for page {:#x}",
            page.start_address().as_u64()
        );
        let frame = crate::memory::frame_allocator::allocate_frame().ok_or("Out of memory")?;
        log::trace!(
            "Allocated frame {:#x} for page {:#x}",
            frame.start_address().as_u64(),
            page.start_address().as_u64()
        );

        unsafe {
            mapper
                .map_to(
                    page,
                    frame,
                    flags,
                    &mut crate::memory::frame_allocator::GlobalFrameAllocator,
                )
                .map_err(|e| {
                    log::error!("Failed to map page at {:?}: {:?}", page.start_address(), e);
                    "Failed to map page"
                })?
                .flush();
        }
    }

    // Copy segment data
    if file_size > 0 {
        let segment_data = &data[file_start..file_start + file_size];
        unsafe {
            core::ptr::copy_nonoverlapping(segment_data.as_ptr(), vaddr.as_mut_ptr(), file_size);
        }
    }

    // Zero remaining memory (BSS)
    if mem_size > file_size {
        let bss_start = user_address(
            vaddr
                .as_u64()
                .checked_add(file_size as u64)
                .ok_or("BSS address overflow")?,
        )?;
        let bss_size = mem_size - file_size;
        unsafe {
            core::ptr::write_bytes(bss_start.as_mut_ptr::<u8>(), 0, bss_size);
        }
    }

    // Now fix the page permissions if the segment is not writable
    if !segment_writable {
        log::trace!("Removing write permission from non-writable segment");
        let correct_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;

        for address in page_addresses.step_by(4096) {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
            unsafe {
                // Update the page table entry to remove write permission
                if mapper.update_flags(page, correct_flags).is_ok() {
                    // Don't flush TLB immediately - let the page table switch handle it
                    // This avoids potential hangs during ELF loading
                    log::trace!("Updated page flags, TLB flush deferred");
                }
            }
        }
    }

    Ok(())
}

/// Load ELF into a specific page table (for process isolation)
pub fn load_elf_into_page_table(
    data: &[u8],
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
) -> Result<LoadedElf, &'static str> {
    if data.len() < mem::size_of::<Elf64Header>() {
        return Err("Data too small for ELF header");
    }

    // Parse ELF header
    let mut header_bytes = [0u8; mem::size_of::<Elf64Header>()];
    header_bytes.copy_from_slice(&data[..mem::size_of::<Elf64Header>()]);
    let header: &Elf64Header = unsafe { &*(header_bytes.as_ptr() as *const Elf64Header) };

    // Validate ELF header
    if header.magic != ELF_MAGIC {
        return Err("Invalid ELF magic");
    }

    if header.class != ELFCLASS64 || header.data != ELFDATA2LSB {
        return Err("Unsupported ELF format");
    }

    log::info!(
        "Loading ELF into process page table: entry={:#x}, {} program headers",
        header.entry,
        header.phnum
    );

    // Track the maximum end of all loaded segments for heap start calculation
    let mut image_size = 0u64;
    let mut data_size = 0u64;
    let mut max_segment_end = 0u64;
    // Track PT_PHDR for auxv
    let mut phdr_vaddr: Option<u64> = None;
    let mut mapped_phdr_vaddr: Option<u64> = None;

    // Load program segments
    for i in 0..header.phnum {
        let ph_start = program_header_offset(header, i as usize, data.len())?;

        // Copy program header to avoid alignment issues
        let mut ph_bytes = [0u8; mem::size_of::<Elf64ProgramHeader>()];
        ph_bytes.copy_from_slice(&data[ph_start..ph_start + mem::size_of::<Elf64ProgramHeader>()]);
        let ph: &Elf64ProgramHeader = unsafe { &*(ph_bytes.as_ptr() as *const Elf64ProgramHeader) };

        // Check for PT_PHDR segment
        if ph.p_type == SegmentType::Phdr as u32 {
            phdr_vaddr = Some(user_address(ph.p_vaddr)?.as_u64());
        }

        if ph.p_type == SegmentType::Load as u32 {
            let bytes = (ph.p_vaddr & 4095)
                .checked_add(ph.p_memsz)
                .and_then(|n| n.checked_add(4095))
                .ok_or("Segment size overflow")?
                & !4095;
            image_size = image_size.checked_add(bytes).ok_or("Image size overflow")?;
            if ph.p_flags & 2 != 0 {
                data_size = data_size.checked_add(bytes).ok_or("Data size overflow")?;
            }
            load_segment_into_page_table(data, ph, page_table)?;
            mapped_phdr_vaddr =
                mapped_phdr_vaddr.or(mapped_program_headers(header, ph, ph.p_vaddr)?);

            // Calculate end of this segment (vaddr + memsz)
            let segment_end = ph
                .p_vaddr
                .checked_add(ph.p_memsz)
                .ok_or("Segment address overflow")?;
            if segment_end > max_segment_end {
                max_segment_end = segment_end;
            }
        }
    }

    // Align heap start to next page boundary (4KB)
    let heap_start = max_segment_end
        .checked_add(0xfff)
        .ok_or("Heap address overflow")?
        & !0xfff;

    // This loader maps absolute p_vaddr addresses (load bias zero).
    let phdr_vaddr = phdr_vaddr.or(mapped_phdr_vaddr).unwrap_or(0);

    log::info!(
        "ELF loaded: segments end at {:#x}, heap will start at {:#x}",
        max_segment_end,
        heap_start
    );

    Ok(LoadedElf {
        entry_point: user_address(header.entry)?,
        stack_top: VirtAddr::zero(), // Stack will be allocated by spawn function
        segments_end: heap_start,
        image_size,
        data_size,
        phdr_vaddr,
        phnum: header.phnum,
        phentsize: header.phentsize,
    })
}

/// Load a program segment into a specific page table
///
/// Linux-style approach: Never switch to process page table during ELF loading.
/// Instead, use physical memory access to write to process pages from kernel space.
/// This prevents page table switching crashes and follows OS-standard practices.
fn load_segment_into_page_table(
    data: &[u8],
    ph: &Elf64ProgramHeader,
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
) -> Result<(), &'static str> {
    // Validate segment
    let file_start = ph.p_offset as usize;
    let file_size = ph.p_filesz as usize;
    let mem_size = ph.p_memsz as usize;

    if file_size > mem_size {
        return Err("Segment file size exceeds memory size");
    }
    if mem_size == 0 {
        return Ok(());
    }

    // Use the virtual address directly - processes have their own address space
    let vaddr = user_address(ph.p_vaddr)?;

    if file_start
        .checked_add(file_size)
        .ok_or("Segment file range overflow")?
        > data.len()
    {
        return Err("Segment data out of bounds");
    }

    log::trace!(
        "Loading segment into page table: vaddr={:#x}, filesz={:#x}, memsz={:#x}, flags={:#x}",
        vaddr.as_u64(),
        file_size,
        mem_size,
        ph.p_flags
    );

    // Calculate pages needed
    let end_addr = user_address(
        vaddr
            .as_u64()
            .checked_add(mem_size as u64 - 1)
            .ok_or("Segment address overflow")?,
    )?;
    let page_addresses = vaddr.align_down(4096u64).as_u64()..=end_addr.as_u64();

    // Determine final permissions
    let segment_writable = ph.p_flags & 2 != 0;
    let segment_executable = ph.p_flags & 1 != 0;

    log::trace!(
        "Segment flags analysis: p_flags={:#x}, writable={}, executable={}",
        ph.p_flags,
        segment_writable,
        segment_executable
    );

    // Set up final page flags
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if segment_writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !segment_executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }

    // Map and load each page - NEVER switch to process page table
    for address in page_addresses.step_by(4096) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
        // Check if page is already mapped (from a previous overlapping segment)
        // This handles cases like RELRO segments that overlap with data segments
        let (frame, already_mapped) =
            if let Some(existing_phys_addr) = page_table.translate_page(page.start_address()) {
                use x86_64::structures::paging::PhysFrame;
                let existing_frame = PhysFrame::containing_address(existing_phys_addr);
                if let Some((_, existing_flags)) = page_table.get_page_info(page) {
                    let mut merged_flags = existing_flags | flags;

                    // x86 NX is restrictive: if any overlapping segment is executable, the
                    // shared page must be executable. This handles ELF layouts where a
                    // page contains the tail of one segment and the head of the next.
                    if !flags.contains(PageTableFlags::NO_EXECUTE) {
                        merged_flags.remove(PageTableFlags::NO_EXECUTE);
                    }

                    if merged_flags != existing_flags {
                        page_table.update_page_flags(page, merged_flags)?;
                    }
                }
                (existing_frame, true)
            } else {
                // Page not mapped yet, allocate a new frame
                let new_frame =
                    crate::memory::frame_allocator::allocate_frame().ok_or("Out of memory")?;

                // Map page in the process page table (from kernel space)
                match page_table.map_page(page, new_frame, flags) {
                    Ok(()) => {}
                    Err(e) => {
                        log::error!("Failed to map page at {:?}: {}", page.start_address(), e);
                        return Err("Failed to map page in process page table");
                    }
                }

                (new_frame, false)
            };

        // Get physical address for direct memory access (Linux-style)
        let physical_memory_offset = crate::memory::physical_memory_offset();
        let frame_phys_addr = frame.start_address();
        let phys_ptr = (physical_memory_offset.as_u64() + frame_phys_addr.as_u64()) as *mut u8;

        // Only clear the page if it wasn't already mapped (i.e., we just allocated it)
        // If it was already mapped, a previous segment's data is there and we don't want to erase it
        if !already_mapped {
            unsafe {
                core::ptr::write_bytes(phys_ptr, 0, 4096);
            }
        }

        // Copy data if this page overlaps with file data
        let page_start_vaddr = page.start_address();

        // Calculate offset within the page where data should go.
        // For the first page of a non-page-aligned segment (p_vaddr not aligned),
        // data starts partway into the page.
        let page_offset = if vaddr > page_start_vaddr {
            (vaddr.as_u64() - page_start_vaddr.as_u64()) as usize
        } else {
            0usize
        };

        // Calculate which part of the file data maps to this page
        let page_file_offset = if page_start_vaddr >= vaddr {
            page_start_vaddr.as_u64() - vaddr.as_u64()
        } else {
            0
        };

        // Limit copy to the remaining space in this page to prevent buffer overflow.
        // Without this, non-page-aligned segments (e.g., BusyBox data at 0x4005ff78)
        // would write past the end of the physical frame, corrupting adjacent memory.
        let bytes_available_in_page = (4096 - page_offset) as u64;
        let copy_start_in_file = page_file_offset;
        let copy_end_in_file =
            core::cmp::min(page_file_offset + bytes_available_in_page, file_size as u64);

        if copy_start_in_file < file_size as u64 && copy_end_in_file > copy_start_in_file {
            let file_data_start = (file_start as u64 + copy_start_in_file) as usize;
            let copy_size = (copy_end_in_file - copy_start_in_file) as usize;

            // Copy using physical memory access (Linux-style approach)
            unsafe {
                let src = data.as_ptr().add(file_data_start);
                let dst = phys_ptr.add(page_offset);
                core::ptr::copy_nonoverlapping(src, dst, copy_size);
            }

            log::trace!(
                "Copied {} bytes to frame {:#x} (page {:#x}) at offset {} using physical access",
                copy_size,
                frame_phys_addr.as_u64(),
                page_start_vaddr.as_u64(),
                page_offset
            );
        }
    }

    log::trace!(
        "Successfully loaded segment with {} pages using Linux-style physical memory access",
        (end_addr.as_u64() - vaddr.align_down(4096u64).as_u64()) / 4096 + 1
    );

    Ok(())
}
