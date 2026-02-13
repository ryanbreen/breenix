//! ARM64 ELF64 loader for userspace programs.
//!
//! This is a minimal ELF loader for ARM64 that loads static executables
//! into the kernel's address space for early testing. Full userspace
//! support will use proper process page tables.

use core::mem;

/// ELF magic number
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF class (64-bit)
pub const ELFCLASS64: u8 = 2;

/// ELF data encoding (little-endian)
pub const ELFDATA2LSB: u8 = 1;

/// ARM64 machine type (EM_AARCH64)
pub const EM_AARCH64: u16 = 0xB7;

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
    Null = 0,
    Load = 1,
    Dynamic = 2,
    Interp = 3,
    Note = 4,
    Shlib = 5,
    Phdr = 6,
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

/// ELF segment flags
pub mod flags {
    pub const PF_X: u32 = 1; // Executable
    pub const PF_W: u32 = 2; // Writable
    pub const PF_R: u32 = 4; // Readable
}

/// Result of loading an ELF binary
#[derive(Debug)]
pub struct LoadedElf {
    /// Entry point address
    pub entry_point: u64,
    /// End of loaded segments (page-aligned, start of heap)
    pub segments_end: u64,
    /// Lowest loaded address
    pub load_base: u64,
}

/// Validate an ELF header for ARM64
pub fn validate_elf_header(data: &[u8]) -> Result<&Elf64Header, &'static str> {
    if data.len() < mem::size_of::<Elf64Header>() {
        return Err("ELF file too small");
    }

    let header = unsafe { &*(data.as_ptr() as *const Elf64Header) };

    // Check magic
    if header.magic != ELF_MAGIC {
        return Err("Invalid ELF magic");
    }

    // Check 64-bit
    if header.class != ELFCLASS64 {
        return Err("Not a 64-bit ELF");
    }

    // Check little-endian
    if header.data != ELFDATA2LSB {
        return Err("Not little-endian ELF");
    }

    // Check executable type
    if header.elf_type != 2 {
        return Err("Not an executable ELF");
    }

    // Check ARM64 machine type
    if header.machine != EM_AARCH64 {
        return Err("Not an ARM64 ELF");
    }

    Ok(header)
}

/// Load an ARM64 ELF binary into memory (kernel space for testing).
///
/// This is a minimal loader that maps segments at their specified virtual
/// addresses. For full userspace support, use process page tables.
///
/// # Safety
///
/// This function writes to arbitrary memory addresses specified in the ELF.
/// Only use with trusted binaries in a controlled environment.
pub unsafe fn load_elf_kernel_space(data: &[u8]) -> Result<LoadedElf, &'static str> {
    let header = validate_elf_header(data)?;

    crate::serial_println!(
        "[elf] Loading ARM64 ELF: entry={:#x}, {} program headers",
        header.entry,
        header.phnum
    );

    let mut max_segment_end: u64 = 0;
    let mut min_load_addr: u64 = u64::MAX;

    // Process program headers
    let ph_offset = header.phoff as usize;
    let ph_size = header.phentsize as usize;
    let ph_count = header.phnum as usize;

    for i in 0..ph_count {
        let ph_start = ph_offset + i * ph_size;
        if ph_start + mem::size_of::<Elf64ProgramHeader>() > data.len() {
            return Err("Program header out of bounds");
        }

        let ph = &*(data.as_ptr().add(ph_start) as *const Elf64ProgramHeader);

        if ph.p_type == SegmentType::Load as u32 {
            load_segment(data, ph)?;

            // Track address range
            if ph.p_vaddr < min_load_addr {
                min_load_addr = ph.p_vaddr;
            }
            let segment_end = ph.p_vaddr + ph.p_memsz;
            if segment_end > max_segment_end {
                max_segment_end = segment_end;
            }
        }
    }

    // Page-align the heap start
    let heap_start = (max_segment_end + 0xfff) & !0xfff;

    crate::serial_println!(
        "[elf] Loaded: base={:#x}, end={:#x}, entry={:#x}",
        min_load_addr,
        heap_start,
        header.entry
    );

    Ok(LoadedElf {
        entry_point: header.entry,
        segments_end: heap_start,
        load_base: if min_load_addr == u64::MAX { 0 } else { min_load_addr },
    })
}

/// Load a single ELF segment into memory.
unsafe fn load_segment(data: &[u8], ph: &Elf64ProgramHeader) -> Result<(), &'static str> {
    let file_start = ph.p_offset as usize;
    let file_size = ph.p_filesz as usize;
    let mem_size = ph.p_memsz as usize;
    let vaddr = ph.p_vaddr;

    if file_start + file_size > data.len() {
        return Err("Segment data out of bounds");
    }

    crate::serial_println!(
        "[elf] Loading segment: vaddr={:#x}, filesz={:#x}, memsz={:#x}, flags={:#x}",
        vaddr,
        file_size,
        mem_size,
        ph.p_flags
    );

    // Copy file data to memory
    if file_size > 0 {
        let src = data.as_ptr().add(file_start);
        let dst = vaddr as *mut u8;
        core::ptr::copy_nonoverlapping(src, dst, file_size);
    }

    // Zero BSS (memory beyond file data)
    if mem_size > file_size {
        let bss_start = (vaddr + file_size as u64) as *mut u8;
        let bss_size = mem_size - file_size;
        core::ptr::write_bytes(bss_start, 0, bss_size);
    }

    Ok(())
}

/// Information about a loaded ELF for userspace execution
#[derive(Debug)]
pub struct UserProgram {
    /// Entry point (PC value)
    pub entry: u64,
    /// Initial stack pointer
    pub stack_top: u64,
    /// Heap base address
    pub heap_base: u64,
}

/// Prepare a simple userspace program for execution.
///
/// This sets up minimal state for running a userspace program:
/// - Entry point from ELF
/// - Stack at a fixed location
/// - Heap base after code/data
pub fn prepare_user_program(elf: &LoadedElf, stack_top: u64) -> UserProgram {
    UserProgram {
        entry: elf.entry_point,
        stack_top,
        heap_base: elf.segments_end,
    }
}

// =============================================================================
// ELF loading into ProcessPageTable (for process isolation)
// =============================================================================

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{
    Page, PageTableFlags, PhysAddr, PhysFrame, Size4KiB, VirtAddr,
};

/// Load ELF into a specific page table (for process isolation)
///
/// This function loads an ARM64 ELF binary into a process's page table,
/// using physical memory access from kernel space (Linux-style approach).
/// The kernel never switches to the process page table during loading.
///
/// # Arguments
/// * `data` - The raw ELF file data
/// * `page_table` - Mutable reference to the process's page table
///
/// # Returns
/// * `Ok(LoadedElf)` - Information about the loaded program
/// * `Err(&'static str)` - Error description if loading failed
pub fn load_elf_into_page_table(
    data: &[u8],
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
) -> Result<LoadedElf, &'static str> {
    // Validate ELF header
    let header = validate_elf_header(data)?;

    log::debug!(
        "[elf-arm64] Loading ELF into process page table: entry={:#x}, {} program headers",
        header.entry,
        header.phnum
    );

    let mut max_segment_end: u64 = 0;
    let mut min_load_addr: u64 = u64::MAX;

    // Process program headers
    let ph_offset = header.phoff as usize;
    let ph_size = header.phentsize as usize;
    let ph_count = header.phnum as usize;

    for i in 0..ph_count {
        let ph_start = ph_offset + i * ph_size;
        if ph_start + mem::size_of::<Elf64ProgramHeader>() > data.len() {
            return Err("Program header out of bounds");
        }

        // Copy program header to avoid alignment issues
        let mut ph_bytes = [0u8; mem::size_of::<Elf64ProgramHeader>()];
        ph_bytes.copy_from_slice(&data[ph_start..ph_start + mem::size_of::<Elf64ProgramHeader>()]);
        let ph: &Elf64ProgramHeader = unsafe { &*(ph_bytes.as_ptr() as *const Elf64ProgramHeader) };

        if ph.p_type == SegmentType::Load as u32 {
            load_segment_into_page_table(data, ph, page_table)?;

            // Track address range
            if ph.p_vaddr < min_load_addr {
                min_load_addr = ph.p_vaddr;
            }
            let segment_end = ph.p_vaddr + ph.p_memsz;
            if segment_end > max_segment_end {
                max_segment_end = segment_end;
            }
        }
    }

    // Page-align the heap start (4KB alignment)
    let heap_start = (max_segment_end + 0xfff) & !0xfff;

    log::debug!(
        "[elf-arm64] Loaded: base={:#x}, end={:#x}, entry={:#x}",
        if min_load_addr == u64::MAX { 0 } else { min_load_addr },
        heap_start,
        header.entry
    );

    Ok(LoadedElf {
        entry_point: header.entry,
        segments_end: heap_start,
        load_base: if min_load_addr == u64::MAX { 0 } else { min_load_addr },
    })
}

/// Load a single ELF segment into a process page table.
///
/// This function:
/// 1. Calculates the page range needed for the segment
/// 2. Allocates physical frames for each page
/// 3. Maps pages into the process page table with appropriate permissions
/// 4. Copies segment data using physical memory access (kernel stays in its own address space)
/// 5. Zeros the BSS region (memsz - filesz)
fn load_segment_into_page_table(
    data: &[u8],
    ph: &Elf64ProgramHeader,
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
) -> Result<(), &'static str> {
    let file_start = ph.p_offset as usize;
    let file_size = ph.p_filesz as usize;
    let mem_size = ph.p_memsz as usize;
    let vaddr = VirtAddr::new(ph.p_vaddr);

    if file_start + file_size > data.len() {
        return Err("Segment data out of bounds");
    }

    log::trace!(
        "[elf-arm64] Loading segment: vaddr={:#x}, filesz={:#x}, memsz={:#x}, flags={:#x}",
        vaddr.as_u64(),
        file_size,
        mem_size,
        ph.p_flags
    );

    // Calculate page range
    let start_page = Page::<Size4KiB>::containing_address(vaddr);
    let end_addr = VirtAddr::new(vaddr.as_u64() + mem_size as u64 - 1);
    let end_page = Page::<Size4KiB>::containing_address(end_addr);

    // Determine page flags based on ELF segment flags
    let segment_readable = ph.p_flags & flags::PF_R != 0;
    let segment_writable = ph.p_flags & flags::PF_W != 0;
    let segment_executable = ph.p_flags & flags::PF_X != 0;

    // All mapped pages need PRESENT and USER_ACCESSIBLE
    let mut page_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;

    if segment_writable {
        page_flags |= PageTableFlags::WRITABLE;
    }

    if !segment_executable {
        page_flags |= PageTableFlags::NO_EXECUTE;
    }

    log::trace!(
        "[elf-arm64] Segment permissions: R={}, W={}, X={}",
        segment_readable,
        segment_writable,
        segment_executable
    );

    // Get physical memory offset for kernel-space access to physical frames
    let physical_memory_offset = crate::memory::physical_memory_offset();

    // Map and load each page
    for page in Page::range_inclusive(start_page, end_page) {
        let page_vaddr = page.start_address();

        // Check if page is already mapped (handles overlapping segments like RELRO)
        // translate_page takes VirtAddr and returns Option<PhysAddr>
        #[cfg(target_arch = "x86_64")]
        let existing_phys_opt = page_table.translate_page(x86_64::VirtAddr::new(page_vaddr.as_u64()));
        #[cfg(not(target_arch = "x86_64"))]
        let existing_phys_opt = page_table.translate_page(crate::memory::arch_stub::VirtAddr::new(page_vaddr.as_u64()));

        let (frame, already_mapped) = if let Some(existing_phys) = existing_phys_opt {
            let existing_frame = PhysFrame::containing_address(PhysAddr::new(existing_phys.as_u64()));
            log::trace!(
                "[elf-arm64] Page {:#x} already mapped to frame {:#x}, reusing",
                page_vaddr.as_u64(),
                existing_frame.start_address().as_u64()
            );
            (existing_frame, true)
        } else {
            // Allocate a new physical frame
            let new_frame = crate::memory::frame_allocator::allocate_frame()
                .ok_or("Out of memory allocating frame for ELF segment")?;

            log::trace!(
                "[elf-arm64] Allocated frame {:#x} for page {:#x}",
                new_frame.start_address().as_u64(),
                page_vaddr.as_u64()
            );

            // Map the page in the process page table
            page_table.map_page(page, new_frame, page_flags)?;

            (new_frame, false)
        };

        // Get virtual pointer to the physical frame via kernel's physical memory mapping
        let frame_phys_addr = frame.start_address();
        let phys_ptr = (physical_memory_offset.as_u64() + frame_phys_addr.as_u64()) as *mut u8;

        // Only zero the page if it was newly allocated (don't overwrite existing data from overlapping segments)
        if !already_mapped {
            unsafe {
                core::ptr::write_bytes(phys_ptr, 0, 4096);
            }
        }

        // Calculate which part of the file data maps to this page
        let page_file_offset = if page_vaddr.as_u64() >= vaddr.as_u64() {
            page_vaddr.as_u64() - vaddr.as_u64()
        } else {
            0
        };

        let copy_start_in_file = page_file_offset;
        let copy_end_in_file = core::cmp::min(page_file_offset + 4096, file_size as u64);

        if copy_start_in_file < file_size as u64 && copy_end_in_file > copy_start_in_file {
            let file_data_start = (file_start as u64 + copy_start_in_file) as usize;
            let copy_size = (copy_end_in_file - copy_start_in_file) as usize;

            // Calculate offset within the page where data should go
            let page_offset = if vaddr.as_u64() > page_vaddr.as_u64() {
                vaddr.as_u64() - page_vaddr.as_u64()
            } else {
                0
            };

            // Copy data using physical memory access (Linux-style approach)
            unsafe {
                let src = data.as_ptr().add(file_data_start);
                let dst = phys_ptr.add(page_offset as usize);
                core::ptr::copy_nonoverlapping(src, dst, copy_size);
            }

            log::trace!(
                "[elf-arm64] Copied {} bytes to frame {:#x} (page {:#x}) at offset {}",
                copy_size,
                frame_phys_addr.as_u64(),
                page_vaddr.as_u64(),
                page_offset
            );
        }
    }

    let page_count = {
        let mut count = 0u64;
        for _ in Page::range_inclusive(start_page, end_page) {
            count += 1;
        }
        count
    };

    log::trace!(
        "[elf-arm64] Successfully loaded segment with {} pages",
        page_count
    );

    Ok(())
}
