//! Kernel ELF loader for UEFI.
//!
//! Reads the kernel-aarch64 ELF binary from the ESP filesystem,
//! loads PT_LOAD segments into physical memory, and locates the
//! `kernel_main` entry point by scanning the ELF symbol table.

use uefi::boot;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::CStr16;

/// Path to the kernel binary on the ESP filesystem.
const KERNEL_PATH: &CStr16 = uefi::cstr16!("\\EFI\\BREENIX\\KERNEL");

/// ELF64 magic bytes.
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF64 header offsets and constants.
const EI_CLASS_64: u8 = 2;
const EI_DATA_LSB: u8 = 1;
const EM_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const STT_FUNC: u8 = 2;

/// Result of loading the kernel.
pub struct LoadedKernel {
    /// Physical address of kernel_main (to jump to after enabling HHDM).
    pub entry_phys: u64,
    /// ELF entry point (virtual address from ELF header, may be _start in boot.S).
    pub elf_entry: u64,
    /// Physical load address of the kernel (lowest PT_LOAD p_paddr).
    pub load_base: u64,
    /// Highest physical address used by kernel (for memory layout).
    pub load_end: u64,
}

/// Load the kernel ELF from the ESP filesystem.
///
/// This must be called while UEFI boot services are still active.
pub fn load_kernel() -> Result<LoadedKernel, &'static str> {
    // Open the ESP filesystem
    let sfs_handle = boot::get_handle_for_protocol::<SimpleFileSystem>()
        .map_err(|_| "No SimpleFileSystem protocol")?;
    let mut sfs = boot::open_protocol_exclusive::<SimpleFileSystem>(sfs_handle)
        .map_err(|_| "Failed to open SimpleFileSystem")?;

    let mut root = sfs.open_volume().map_err(|_| "Failed to open ESP volume")?;

    // Open the kernel file
    let file_handle = root
        .open(KERNEL_PATH, FileMode::Read, FileAttribute::empty())
        .map_err(|_| "Failed to open kernel file (\\EFI\\BREENIX\\KERNEL)")?;

    let mut file = file_handle
        .into_regular_file()
        .ok_or("Kernel path is not a regular file")?;

    // Get file size
    let mut info_buf = [0u8; 512];
    let info = file
        .get_info::<FileInfo>(&mut info_buf)
        .map_err(|_| "Failed to get kernel file info")?;
    let file_size = info.file_size() as usize;

    log::info!("Kernel file size: {} bytes ({} KB)", file_size, file_size / 1024);

    if file_size < 64 {
        return Err("Kernel file too small for ELF header");
    }

    // Allocate buffer and read the entire file
    // Use UEFI pool allocation which is available during boot services
    let elf_data = boot::allocate_pool(uefi::mem::memory_map::MemoryType::LOADER_DATA, file_size)
        .map_err(|_| "Failed to allocate memory for kernel")?;

    let elf_buf = unsafe { core::slice::from_raw_parts_mut(elf_data.as_ptr(), file_size) };

    let bytes_read = file.read(elf_buf).map_err(|_| "Failed to read kernel file")?;
    if bytes_read != file_size {
        return Err("Incomplete read of kernel file");
    }

    // Parse and load the ELF
    let result = parse_and_load_elf(elf_buf);

    // Free the file buffer (segments are already copied to physical memory)
    unsafe {
        boot::free_pool(elf_data).ok();
    }

    result
}

/// Parse the ELF header, load PT_LOAD segments, and find kernel_main.
fn parse_and_load_elf(elf: &[u8]) -> Result<LoadedKernel, &'static str> {
    // Validate ELF magic
    if elf.len() < 64 || elf[0..4] != ELF_MAGIC {
        return Err("Invalid ELF magic");
    }
    if elf[4] != EI_CLASS_64 {
        return Err("Not ELF64");
    }
    if elf[5] != EI_DATA_LSB {
        return Err("Not little-endian");
    }

    let e_machine = read_u16(elf, 18);
    if e_machine != EM_AARCH64 {
        return Err("Not AArch64 ELF");
    }

    let e_entry = read_u64(elf, 24);
    let e_phoff = read_u64(elf, 32) as usize;
    let e_shoff = read_u64(elf, 40) as usize;
    let e_phentsize = read_u16(elf, 54) as usize;
    let e_phnum = read_u16(elf, 56) as usize;
    let e_shentsize = read_u16(elf, 58) as usize;
    let e_shnum = read_u16(elf, 60) as usize;
    let e_shstrndx = read_u16(elf, 62) as usize;

    log::info!(
        "ELF: entry={:#x}, {} phdrs, {} shdrs",
        e_entry, e_phnum, e_shnum
    );

    // Load PT_LOAD segments into physical memory
    let mut load_base = u64::MAX;
    let mut load_end = 0u64;
    let mut vaddr_to_paddr_offset: i64 = 0;

    for i in 0..e_phnum {
        let ph_offset = e_phoff + i * e_phentsize;
        if ph_offset + e_phentsize > elf.len() {
            continue;
        }

        let p_type = read_u32(elf, ph_offset);
        if p_type != PT_LOAD {
            continue;
        }

        let p_offset = read_u64(elf, ph_offset + 8) as usize;
        let p_vaddr = read_u64(elf, ph_offset + 16);
        let p_paddr = read_u64(elf, ph_offset + 24);
        let p_filesz = read_u64(elf, ph_offset + 32) as usize;
        let p_memsz = read_u64(elf, ph_offset + 40) as usize;

        log::info!(
            "  LOAD: vaddr={:#x} paddr={:#x} filesz={:#x} memsz={:#x}",
            p_vaddr, p_paddr, p_filesz, p_memsz
        );

        // Track the vaddr→paddr mapping for symbol resolution
        if p_filesz > 0 {
            vaddr_to_paddr_offset = p_paddr as i64 - p_vaddr as i64;
        }

        // Copy file data to physical address
        if p_filesz > 0 {
            let src = &elf[p_offset..p_offset + p_filesz];
            let dst = p_paddr as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), dst, p_filesz);
            }
        }

        // Zero BSS (memsz > filesz)
        if p_memsz > p_filesz {
            let bss_start = (p_paddr + p_filesz as u64) as *mut u8;
            let bss_size = p_memsz - p_filesz;
            unsafe {
                core::ptr::write_bytes(bss_start, 0, bss_size);
            }
        }

        load_base = load_base.min(p_paddr);
        load_end = load_end.max(p_paddr + p_memsz as u64);
    }

    if load_base == u64::MAX {
        return Err("No PT_LOAD segments found");
    }

    log::info!("Kernel loaded at phys {:#x}-{:#x}", load_base, load_end);

    // Try to find kernel_main symbol
    let kernel_main_phys = find_symbol(elf, "kernel_main", e_shoff, e_shentsize, e_shnum, e_shstrndx)
        .map(|vaddr| (vaddr as i64 + vaddr_to_paddr_offset) as u64);

    let entry_phys = match kernel_main_phys {
        Some(addr) => {
            log::info!("Found kernel_main at phys {:#x}", addr);
            addr
        }
        None => {
            // Fallback: use ELF entry point with vaddr→paddr translation
            let fallback = (e_entry as i64 + vaddr_to_paddr_offset) as u64;
            log::warn!("kernel_main not found, using ELF entry {:#x} (phys {:#x})", e_entry, fallback);
            fallback
        }
    };

    Ok(LoadedKernel {
        entry_phys,
        elf_entry: e_entry,
        load_base,
        load_end,
    })
}

/// Find a symbol by name in the ELF symbol table.
///
/// Returns the symbol's virtual address (st_value) if found.
fn find_symbol(
    elf: &[u8],
    name: &str,
    e_shoff: usize,
    e_shentsize: usize,
    e_shnum: usize,
    e_shstrndx: usize,
) -> Option<u64> {
    if e_shoff == 0 || e_shnum == 0 {
        return None;
    }

    // First, find the section name string table
    let shstr_offset = e_shoff + e_shstrndx * e_shentsize;
    if shstr_offset + e_shentsize > elf.len() {
        return None;
    }
    let _shstr_sh_offset = read_u64(elf, shstr_offset + 24) as usize;
    let _shstr_sh_size = read_u64(elf, shstr_offset + 32) as usize;

    // Find .symtab and its associated .strtab
    let mut symtab_offset = 0usize;
    let mut symtab_size = 0usize;
    let mut symtab_entsize = 0usize;
    let mut symtab_link = 0u32; // Index of associated string table
    let mut strtab_offset = 0usize;
    let mut _strtab_size = 0usize;

    for i in 0..e_shnum {
        let sh = e_shoff + i * e_shentsize;
        if sh + e_shentsize > elf.len() {
            continue;
        }

        let sh_type = read_u32(elf, sh + 4);
        if sh_type == SHT_SYMTAB {
            symtab_offset = read_u64(elf, sh + 24) as usize;
            symtab_size = read_u64(elf, sh + 32) as usize;
            symtab_entsize = read_u64(elf, sh + 56) as usize;
            symtab_link = read_u32(elf, sh + 12);
        }
    }

    if symtab_offset == 0 || symtab_entsize == 0 {
        // No symbol table - try .dynsym
        for i in 0..e_shnum {
            let sh = e_shoff + i * e_shentsize;
            if sh + e_shentsize > elf.len() {
                continue;
            }
            let sh_name_idx = read_u32(elf, sh) as usize;
            let sh_type = read_u32(elf, sh + 4);

            // Check if this is .dynsym
            if sh_type == 11 {
                // SHT_DYNSYM
                symtab_offset = read_u64(elf, sh + 24) as usize;
                symtab_size = read_u64(elf, sh + 32) as usize;
                symtab_entsize = read_u64(elf, sh + 56) as usize;
                symtab_link = read_u32(elf, sh + 12);
            }
            let _ = sh_name_idx; // Suppress warning
        }
    }

    if symtab_offset == 0 || symtab_entsize == 0 {
        return None;
    }

    // Find the associated string table
    let strtab_sh = e_shoff + (symtab_link as usize) * e_shentsize;
    if strtab_sh + e_shentsize <= elf.len() {
        strtab_offset = read_u64(elf, strtab_sh + 24) as usize;
        _strtab_size = read_u64(elf, strtab_sh + 32) as usize;
    }

    if strtab_offset == 0 {
        return None;
    }

    // Iterate symbols looking for kernel_main
    let num_symbols = symtab_size / symtab_entsize;
    for i in 0..num_symbols {
        let sym = symtab_offset + i * symtab_entsize;
        if sym + symtab_entsize > elf.len() {
            break;
        }

        let st_name = read_u32(elf, sym) as usize;
        let st_info = elf[sym + 4];
        let st_value = read_u64(elf, sym + 8);

        // Check if it's a function
        let st_type = st_info & 0xF;
        if st_type != STT_FUNC || st_value == 0 {
            continue;
        }

        // Compare name
        let name_offset = strtab_offset + st_name;
        if name_offset >= elf.len() {
            continue;
        }

        let sym_name = read_cstr(elf, name_offset);
        if sym_name == name {
            return Some(st_value);
        }
    }

    None
}

/// Read a null-terminated string from a byte slice.
fn read_cstr(data: &[u8], offset: usize) -> &str {
    let start = offset;
    let mut end = start;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    core::str::from_utf8(&data[start..end]).unwrap_or("")
}

// Little-endian read helpers
fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
    ])
}
