//! Discover the static executable's unwind index using K1's dl_iterate_phdr.
//! Program headers follow ELF gABI §7 and LSB Core 5.0 §11.2, Table 11-1.
//! The callback prefix follows LSB Core 5.0 §14.17 (dl_iterate_phdr).

use core::ffi::{c_char, c_int, c_void};
use core::mem::size_of;
use core::ops::Range;
use core::{ptr, slice};

use crate::cfi::Endian;
use crate::ehframe::{self, FrameSource, Memory, Section, Sections};

const PT_LOAD: u32 = 1;
const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const ADDRESS_SIZE: usize = size_of::<usize>();

#[repr(C)]
struct ProgramHeader {
    kind: u32,
    flags: u32,
    offset: u64,
    virtual_address: u64,
    physical_address: u64,
    file_size: u64,
    memory_size: u64,
    alignment: u64,
}

#[repr(C)]
struct PhdrInfo {
    load_bias: u64,
    name: *const c_char,
    headers: *const ProgramHeader,
    header_count: u16,
}

type Visit = unsafe extern "C" fn(*mut PhdrInfo, usize, *mut c_void) -> c_int;

extern "C" {
    static __breenix_text_start: u8;
    static __breenix_got_start: u8;
    fn dl_iterate_phdr(visit: Visit, data: *mut c_void) -> c_int;
}

pub(crate) struct LoadedImage {
    load_bias: u64,
    headers: &'static [ProgramHeader],
}

impl LoadedImage {
    pub(crate) fn discover() -> Option<Self> {
        let mut image: Option<Self> = None;
        // SAFETY: K1 calls visit synchronously with its live record and the
        // supplied pointer to the local Option.
        unsafe { dl_iterate_phdr(visit, ptr::from_mut(&mut image).cast()) };
        image
    }

    fn span(&self, header: &ProgramHeader) -> Option<Range<u64>> {
        let start = self.load_bias.checked_add(header.virtual_address)?;
        if start == 0 || header.file_size > header.memory_size {
            return None;
        }
        Some(start..start.checked_add(header.file_size)?)
    }

    fn containing_load(&self, addr: u64, required: u32, forbidden: u32) -> Option<Range<u64>> {
        self.headers
            .iter()
            .filter(|h| {
                h.kind == PT_LOAD && h.flags & required == required && h.flags & forbidden == 0
            })
            .filter_map(|h| self.span(h))
            .find(|span| span.contains(&addr))
    }

    pub(crate) fn is_code(&self, addr: u64) -> bool {
        self.containing_load(addr, PF_X, 0).is_some()
    }
}

unsafe extern "C" fn visit(info: *mut PhdrInfo, size: usize, data: *mut c_void) -> c_int {
    if size < size_of::<PhdrInfo>() || info.is_null() {
        return 0;
    }
    // SAFETY: K1 supplies the aligned, live LSB record for this callback.
    let info = unsafe { &*info };
    if info.headers.is_null() || !info.headers.is_aligned() || info.header_count == 0 {
        return 0;
    }
    // SAFETY: K1 derives this pointer and count from the auxiliary vector.
    // The main executable's program headers remain mapped for its lifetime.
    let headers = unsafe { slice::from_raw_parts(info.headers, usize::from(info.header_count)) };
    if !headers.iter().any(|h| h.kind == PT_GNU_EH_FRAME) {
        return 0;
    }
    // SAFETY: discover supplied a live, exclusive Option<LoadedImage>.
    let image = unsafe { &mut *data.cast::<Option<LoadedImage>>() };
    *image = Some(LoadedImage {
        load_bias: info.load_bias,
        headers,
    });
    1
}

impl Memory for LoadedImage {
    fn read(&self, address: u64, size: usize) -> Option<&[u8]> {
        let span = self.containing_load(address, PF_R, PF_W)?;
        if size > isize::MAX as usize || address.checked_add(size as u64)? > span.end {
            return None;
        }
        let bytes = ptr::with_exposed_provenance(address as usize);
        // SAFETY: The complete span is within an immutable file-backed load
        // of the static executable and its length fits Rust's slice bound.
        Some(unsafe { slice::from_raw_parts(bytes, size) })
    }
}

impl FrameSource for LoadedImage {
    fn sections_for_pc(&self, pc: u64) -> Option<Sections<'_>> {
        if !self.is_code(pc) {
            return None;
        }
        let index = self.headers.iter().find(|h| h.kind == PT_GNU_EH_FRAME)?;
        let address = self.span(index)?.start;
        let header = Section {
            address,
            bytes: self.read(address, usize::try_from(index.file_size).ok()?)?,
        };
        let frame_address =
            ehframe::frame_address(header, self, ADDRESS_SIZE, Endian::Little).ok()?;
        let load = self.containing_load(frame_address, PF_R, PF_W)?;
        let bytes = self.read(
            frame_address,
            usize::try_from(load.end - frame_address).ok()?,
        )?;
        Some(Sections {
            frame: Section {
                address: frame_address,
                bytes,
            },
            header: Some(header),
            text_base: Some(ptr::addr_of!(__breenix_text_start).expose_provenance() as u64),
            data_base: match ptr::addr_of!(__breenix_got_start).expose_provenance() as u64 {
                0 => None,
                value => Some(value),
            },
            address_size: ADDRESS_SIZE as u8,
            endian: Endian::Little,
        })
    }
}
