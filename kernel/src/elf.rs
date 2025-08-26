//! ELF64 loader for executing userspace programs

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

/// ELF loader result
pub struct LoadedElf {
    pub entry_point: VirtAddr,
    pub stack_top: VirtAddr,
}

/// Load an ELF64 binary into memory
pub fn load_elf(data: &[u8]) -> Result<LoadedElf, &'static str> {
    load_elf_at_base(data, VirtAddr::zero())
}

/// Load an ELF64 binary into memory with a base address offset
pub fn load_elf_at_base(data: &[u8], base_offset: VirtAddr) -> Result<LoadedElf, &'static str> {
    log::debug!(
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

    log::debug!("ELF header loaded");

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
            load_segment(data, ph, base_offset)?;
        }
    }

    // The entry point should be the header entry point directly
    // since our userspace binaries are compiled with absolute addresses
    Ok(LoadedElf {
        entry_point: VirtAddr::new(header.entry),
        stack_top: VirtAddr::zero(), // Stack will be allocated by spawn function
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

    // Our userspace binaries use absolute addressing starting at 0x10000000
    // Don't add base_offset for absolute addresses in the userspace range
    let vaddr = if ph.p_vaddr >= 0x10000000 {
        // Absolute userspace address - use directly
        VirtAddr::new(ph.p_vaddr)
    } else {
        // Relative address - add base offset
        base_offset + ph.p_vaddr
    };

    if file_start + file_size > data.len() {
        return Err("Segment data out of bounds");
    }

    log::debug!(
        "Loading segment: vaddr={:#x}, filesz={:#x}, memsz={:#x}, flags={:#x}",
        vaddr.as_u64(),
        file_size,
        mem_size,
        ph.p_flags
    );

    // Calculate pages needed
    let start_page = Page::<Size4KiB>::containing_address(vaddr);
    let end_addr = vaddr + mem_size as u64 - 1u64;
    let end_page = Page::<Size4KiB>::containing_address(end_addr);

    // Map pages
    let mut mapper = unsafe { crate::memory::paging::get_mapper() };

    // Initially map all pages as writable so we can load the data
    // We'll fix permissions later if needed
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;

    let segment_writable = ph.p_flags & 2 != 0;
    let segment_executable = ph.p_flags & 1 != 0;

    log::debug!(
        "Segment permissions: readable={}, writable={}, executable={}",
        ph.p_flags & 4 != 0,
        segment_writable,
        segment_executable
    );

    // Map all pages for the segment
    for page in Page::range_inclusive(start_page, end_page) {
        log::debug!(
            "Allocating frame for page {:#x}",
            page.start_address().as_u64()
        );
        let frame = crate::memory::frame_allocator::allocate_frame().ok_or("Out of memory")?;
        log::debug!(
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
        let bss_start = vaddr + file_size as u64;
        let bss_size = mem_size - file_size;
        unsafe {
            core::ptr::write_bytes(bss_start.as_mut_ptr::<u8>(), 0, bss_size);
        }
    }

    // Now fix the page permissions if the segment is not writable
    if !segment_writable {
        log::debug!("Removing write permission from non-writable segment");
        let correct_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;

        for page in Page::range_inclusive(start_page, end_page) {
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

    // Load program segments
    for i in 0..header.phnum {
        let ph_offset = header.phoff as usize + (i as usize * mem::size_of::<Elf64ProgramHeader>());

        if ph_offset + mem::size_of::<Elf64ProgramHeader>() > data.len() {
            return Err("Program header out of bounds");
        }

        let ph_start = ph_offset;

        // Copy program header to avoid alignment issues
        let mut ph_bytes = [0u8; mem::size_of::<Elf64ProgramHeader>()];
        ph_bytes.copy_from_slice(&data[ph_start..ph_start + mem::size_of::<Elf64ProgramHeader>()]);
        let ph: &Elf64ProgramHeader = unsafe { &*(ph_bytes.as_ptr() as *const Elf64ProgramHeader) };

        if ph.p_type == SegmentType::Load as u32 {
            load_segment_into_page_table(data, ph, page_table)?;
        }
    }

    Ok(LoadedElf {
        entry_point: VirtAddr::new(header.entry),
        stack_top: VirtAddr::zero(), // Stack will be allocated by spawn function
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

    // Use the virtual address directly - processes have their own address space
    let vaddr = VirtAddr::new(ph.p_vaddr);

    if file_start + file_size > data.len() {
        return Err("Segment data out of bounds");
    }

    log::debug!(
        "Loading segment into page table: vaddr={:#x}, filesz={:#x}, memsz={:#x}, flags={:#x}",
        vaddr.as_u64(),
        file_size,
        mem_size,
        ph.p_flags
    );

    // Calculate pages needed
    let start_page = Page::<Size4KiB>::containing_address(vaddr);
    let end_addr = vaddr + mem_size as u64 - 1u64;
    let end_page = Page::<Size4KiB>::containing_address(end_addr);

    // Determine final permissions
    let segment_writable = ph.p_flags & 2 != 0;
    let segment_executable = ph.p_flags & 1 != 0;

    // Set up final page flags
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if segment_writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !segment_executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }

    log::debug!("Linux-style ELF loading: staying in kernel space, using physical memory access");

    // Map and load each page - NEVER switch to process page table
    for page in Page::range_inclusive(start_page, end_page) {
        log::debug!("Processing page {:#x}", page.start_address().as_u64());
        let frame = crate::memory::frame_allocator::allocate_frame().ok_or("Out of memory")?;
        log::debug!(
            "Allocated frame {:#x} for page {:#x}",
            frame.start_address().as_u64(),
            page.start_address().as_u64()
        );

        // Map page in the process page table (from kernel space)
        log::debug!(
            "Mapping page {:#x} to frame {:#x} with flags {:?}",
            page.start_address().as_u64(),
            frame.start_address().as_u64(),
            flags
        );
        log::debug!("About to call page_table.map_page...");
        match page_table.map_page(page, frame, flags) {
            Ok(()) => {
                log::debug!(
                    "Successfully mapped page {:#x}",
                    page.start_address().as_u64()
                );
            }
            Err(e) => {
                log::error!("Failed to map page at {:?}: {}", page.start_address(), e);
                return Err("Failed to map page in process page table");
            }
        }
        log::debug!("After page_table.map_page");

        // Get physical address for direct memory access (Linux-style)
        let physical_memory_offset = crate::memory::physical_memory_offset();
        let frame_phys_addr = frame.start_address();
        let phys_ptr = (physical_memory_offset.as_u64() + frame_phys_addr.as_u64()) as *mut u8;

        // Clear the entire page using physical memory access
        unsafe {
            core::ptr::write_bytes(phys_ptr, 0, 4096);
        }

        // Copy data if this page overlaps with file data
        let page_start_vaddr = page.start_address();

        // Calculate which part of the file data maps to this page
        let page_file_offset = if page_start_vaddr >= vaddr {
            page_start_vaddr.as_u64() - vaddr.as_u64()
        } else {
            0
        };

        let copy_start_in_file = page_file_offset;
        let copy_end_in_file = core::cmp::min(page_file_offset + 4096, file_size as u64);

        if copy_start_in_file < file_size as u64 && copy_end_in_file > copy_start_in_file {
            let file_data_start = (file_start as u64 + copy_start_in_file) as usize;
            let copy_size = (copy_end_in_file - copy_start_in_file) as usize;

            // Calculate offset within the page where data should go
            let page_offset = if vaddr > page_start_vaddr {
                vaddr.as_u64() - page_start_vaddr.as_u64()
            } else {
                0
            };

            // Copy using physical memory access (Linux-style approach)
            unsafe {
                let src = data.as_ptr().add(file_data_start);
                let dst = phys_ptr.add(page_offset as usize);
                core::ptr::copy_nonoverlapping(src, dst, copy_size);
            }

            log::debug!(
                "Copied {} bytes to frame {:#x} (page {:#x}) at offset {} using physical access",
                copy_size,
                frame_phys_addr.as_u64(),
                page_start_vaddr.as_u64(),
                page_offset
            );
        }
    }

    log::debug!(
        "Successfully loaded segment with {} pages using Linux-style physical memory access",
        Page::range_inclusive(start_page, end_page).count()
    );

    Ok(())
}
