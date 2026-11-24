//! Static executable enumeration, from LSB Core 5.0 and the ELF gABI.
#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::{c_char, c_int, c_ulonglong, c_void};
use core::mem::size_of;
use core::ptr;

// ELF gABI, Program Header and Thread-Local Storage.
const PT_TLS: u32 = 7;
// System V AMD64 psABI, Auxiliary Vector (also used by Breenix on AArch64).
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;

#[repr(C)]
pub(crate) struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

// LSB Core 5.0, Generic Part dl_iterate_phdr and AMD64 §10.3.20.
#[repr(C)]
pub(crate) struct DlPhdrInfo {
    pub dlpi_addr: u64,
    pub dlpi_name: *const c_char,
    pub dlpi_phdr: *const Elf64Phdr,
    pub dlpi_phnum: u16,
    pub dlpi_adds: c_ulonglong,
    pub dlpi_subs: c_ulonglong,
    pub dlpi_tls_modid: usize,
    pub dlpi_tls_data: *mut c_void,
}

type Callback = unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int;

/// Visit the main executable of a statically linked process.
///
/// # Safety
///
/// The callback must be callable with the supplied context. Its record pointer
/// is valid for the duration of the callback.
#[cfg_attr(not(test), no_mangle)]
pub(crate) unsafe extern "C" fn dl_iterate_phdr(callback: Callback, data: *mut c_void) -> c_int {
    let (Some(address), Some(count)) = (
        crate::crt::auxv_entry(AT_PHDR),
        crate::crt::auxv_entry(AT_PHNUM),
    ) else {
        return 0;
    };
    let Ok(count) = u16::try_from(count) else {
        return 0;
    };
    let phdr = address as *const Elf64Phdr;
    if phdr.is_null()
        || !phdr.is_aligned()
        || count == 0
        || crate::crt::auxv_entry(AT_PHENT)
            .is_some_and(|size| size != size_of::<Elf64Phdr>() as u64)
    {
        return 0;
    }

    // SAFETY: the executable's auxiliary vector gives the address and element
    // count of its mapped ELF64 program header table.
    let headers = unsafe { core::slice::from_raw_parts(phdr, usize::from(count)) };
    let has_tls = headers.iter().any(|header| header.p_type == PT_TLS);
    let mut info = DlPhdrInfo {
        dlpi_addr: 0,
        dlpi_name: c"".as_ptr(),
        dlpi_phdr: phdr,
        dlpi_phnum: count,
        dlpi_adds: 1,
        dlpi_subs: 0,
        dlpi_tls_modid: usize::from(has_tls),
        dlpi_tls_data: if has_tls {
            crate::pthread::static_tls_data()
        } else {
            ptr::null_mut()
        },
    };
    // SAFETY: the caller supplies the callback and context, and info is live.
    unsafe { callback(&mut info, size_of::<DlPhdrInfo>(), data) }
}
