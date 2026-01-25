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
