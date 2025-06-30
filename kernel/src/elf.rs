//! ELF64 loader for executing userspace programs

use core::mem;
use x86_64::{VirtAddr, structures::paging::{Page, PageTableFlags, Size4KiB, Mapper}};

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
    log::debug!("load_elf: data size = {} bytes", data.len());
    
    // Verify ELF header
    if data.len() < mem::size_of::<Elf64Header>() {
        log::error!("ELF file too small: {} < {}", data.len(), mem::size_of::<Elf64Header>());
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
    
    log::info!("Loading ELF: entry={:#x}, {} program headers", 
        header.entry, header.phnum);
    
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
            load_segment(data, ph)?;
        }
    }
    
    Ok(LoadedElf {
        entry_point: VirtAddr::new(header.entry),
        stack_top: VirtAddr::zero(), // Stack will be allocated by spawn function
    })
}

/// Load a program segment into memory
fn load_segment(data: &[u8], ph: &Elf64ProgramHeader) -> Result<(), &'static str> {
    // Validate segment
    let file_start = ph.p_offset as usize;
    let file_size = ph.p_filesz as usize;
    let mem_size = ph.p_memsz as usize;
    let vaddr = VirtAddr::new(ph.p_vaddr);
    
    if file_start + file_size > data.len() {
        return Err("Segment data out of bounds");
    }
    
    log::debug!("Loading segment: vaddr={:#x}, filesz={:#x}, memsz={:#x}, flags={:#x}", 
        vaddr.as_u64(), file_size, mem_size, ph.p_flags);
    
    // Calculate pages needed
    let start_page = Page::<Size4KiB>::containing_address(vaddr);
    let end_addr = vaddr + mem_size as u64 - 1u64;
    let end_page = Page::<Size4KiB>::containing_address(end_addr);
    
    // Map pages
    let mut mapper = unsafe { crate::memory::paging::get_mapper() };
    
    // Initially map all pages as writable so we can load the data
    // We'll fix permissions later if needed
    let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
    
    let segment_writable = ph.p_flags & 2 != 0;
    let segment_executable = ph.p_flags & 1 != 0;
    
    log::debug!("Segment permissions: readable={}, writable={}, executable={}", 
        ph.p_flags & 4 != 0, segment_writable, segment_executable);
    
    // Map all pages for the segment
    for page in Page::range_inclusive(start_page, end_page) {
        let frame = crate::memory::frame_allocator::allocate_frame()
            .ok_or("Out of memory")?;
        
        unsafe {
            mapper.map_to(
                page, 
                frame, 
                flags, 
                &mut crate::memory::frame_allocator::GlobalFrameAllocator
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
            core::ptr::copy_nonoverlapping(
                segment_data.as_ptr(),
                vaddr.as_mut_ptr(),
                file_size
            );
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
                    // Flush TLB for this page
                    x86_64::instructions::tlb::flush(page.start_address());
                }
            }
        }
    }
    
    Ok(())
}