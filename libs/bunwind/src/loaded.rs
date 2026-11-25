//! Find a frame's ELF object and unwind index using LSB Core 5.0 §14.17.
//! Program headers follow ELF gABI §7 and LSB Core 5.0 §11.2, Table 11-1.

use core::ffi::c_char;
#[cfg(target_os = "breenix")]
use core::ffi::c_void;
use core::mem::size_of;
use core::ops::Range;
use core::{ptr, slice};

use crate::cfi::{self, Endian};
use crate::ehframe::{self, FrameSource, Memory, Section, Sections};

const PT_LOAD: u32 = 1;
const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const ADDRESS_SIZE: usize = size_of::<usize>();
const BASES_OFFSET: usize = 64;

#[repr(C)]
pub(crate) struct ProgramHeader {
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
struct PhdrPrefix {
    load_bias: u64,
    name: *const c_char,
    headers: *const ProgramHeader,
    header_count: u16,
}

#[repr(C)]
pub(crate) struct PhdrInfo {
    prefix: PhdrPrefix,
    lsb_fields: [u64; 4],
    bases: [u64; 2],
}

#[cfg(target_os = "breenix")]
extern "C" {
    static __breenix_text_start: u8;
    static __breenix_got_start: u8;
    fn dl_iterate_phdr(
        visit: unsafe extern "C" fn(*mut PhdrInfo, usize, *mut c_void) -> i32,
        data: *mut c_void,
    ) -> i32;
}

pub(crate) struct LoadedImage<'a> {
    #[cfg(target_os = "breenix")]
    name: *const c_char,
    load_bias: u64,
    headers: &'a [ProgramHeader],
    bases: [u64; 2],
}

impl<'a> LoadedImage<'a> {
    /// The header table and described loads must remain mapped for 'a.
    #[cfg(test)]
    pub(crate) unsafe fn for_pc(pc: u64, info: *const PhdrInfo, size: usize) -> Option<Self> {
        // SAFETY: the caller supplies a suitably aligned callback record of size bytes.
        let image = unsafe { Self::from_info(info, size) }?;
        image.containing_load(pc, PF_X, 0).map(|_| image)
    }

    unsafe fn from_info(info: *const PhdrInfo, size: usize) -> Option<Self> {
        if size < size_of::<PhdrPrefix>() || info.is_null() {
            return None;
        }
        // SAFETY: the callback record includes this common LSB prefix.
        let prefix = unsafe { &*info.cast::<PhdrPrefix>() };
        if prefix.headers.is_null() || !prefix.headers.is_aligned() || prefix.header_count == 0 {
            return None;
        }
        // SAFETY: dl_iterate_phdr supplies this table, resident for 'a.
        let headers =
            unsafe { slice::from_raw_parts(prefix.headers, usize::from(prefix.header_count)) };
        let bases = if size >= BASES_OFFSET + size_of::<[u64; 2]>() {
            // SAFETY: the reported size covers the Breenix extension.
            unsafe { ptr::addr_of!((*info).bases).read() }
        } else {
            static_bases()
        };
        Some(Self {
            #[cfg(target_os = "breenix")]
            name: prefix.name,
            load_bias: prefix.load_bias,
            headers,
            bases,
        })
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

    fn read_mapped(&self, address: u64, size: usize) -> Option<&'a [u8]> {
        let span = self.containing_load(address, PF_R, PF_W)?;
        if size > isize::MAX as usize || address.checked_add(size as u64)? > span.end {
            return None;
        }
        let bytes = ptr::with_exposed_provenance(address as usize);
        // SAFETY: The complete span lies in this object's read-only file bytes.
        Some(unsafe { slice::from_raw_parts(bytes, size) })
    }

    fn sections(&self) -> Option<Sections<'a>> {
        let index = self.headers.iter().find(|h| h.kind == PT_GNU_EH_FRAME)?;
        let address = self.span(index)?.start;
        let header = Section {
            address,
            bytes: self.read_mapped(address, usize::try_from(index.file_size).ok()?)?,
        };
        let frame_address =
            ehframe::frame_address(header, self, ADDRESS_SIZE, Endian::Little).ok()?;
        let load = self.containing_load(frame_address, PF_R, PF_W)?;
        let bytes = self.read_mapped(
            frame_address,
            usize::try_from(load.end - frame_address).ok()?,
        )?;
        Some(Sections {
            frame: Section {
                address: frame_address,
                bytes,
            },
            header: Some(header),
            text_base: (self.bases[0] != 0).then_some(self.bases[0]),
            data_base: (self.bases[1] != 0).then_some(self.bases[1]),
            address_size: ADDRESS_SIZE as u8,
            endian: Endian::Little,
        })
    }
}

fn static_bases() -> [u64; 2] {
    #[cfg(target_os = "breenix")]
    return [
        ptr::addr_of!(__breenix_text_start).expose_provenance() as u64,
        ptr::addr_of!(__breenix_got_start).expose_provenance() as u64,
    ];
    #[cfg(not(target_os = "breenix"))]
    [0; 2]
}

impl Memory for LoadedImage<'_> {
    fn read(&self, address: u64, size: usize) -> Option<&[u8]> {
        self.read_mapped(address, size)
    }

    fn read_indirect(&self, address: u64, size: usize, endian: Endian) -> Option<u64> {
        let span = self.containing_load(address, PF_R, 0)?;
        if !matches!(size, 4 | 8) || address.checked_add(size as u64)? > span.end {
            return None;
        }
        let mut bytes = [0u8; 8];
        let slot = ptr::with_exposed_provenance(address as usize);
        // SAFETY: this file-backed span contains a compiler-emitted pointer
        // slot fixed by relocation before the object became visible.
        unsafe { ptr::copy_nonoverlapping(slot, bytes.as_mut_ptr(), size) };
        cfi::uint(&mut &bytes[..size], size, endian).ok()
    }
}

#[cfg(test)]
impl FrameSource for LoadedImage<'_> {
    fn sections_for_pc(&self, pc: u64) -> Option<Sections<'_>> {
        self.containing_load(pc, PF_X, 0)?;
        self.sections()
    }
}

#[cfg(target_os = "breenix")]
pub(crate) struct LoadedObjects;

#[cfg(target_os = "breenix")]
impl LoadedObjects {
    pub(crate) fn is_code(address: u64) -> bool {
        Self::find(address, PF_X, 0).is_some()
    }

    /// The name and load bias of the object with a load containing `address`.
    pub(crate) fn origin(address: u64) -> Option<(&'static [u8], u64)> {
        let image = Self::find(address, 0, 0)?;
        let name = if image.name.is_null() {
            &[][..]
        } else {
            // SAFETY: dl_iterate_phdr names are C strings that stay resident:
            // the loader keeps each published object's name for its lifetime.
            unsafe { core::ffi::CStr::from_ptr(image.name) }.to_bytes()
        };
        Some((if name.is_empty() { b"(main program)" } else { name }, image.load_bias))
    }

    fn find(address: u64, required: u32, forbidden: u32) -> Option<LoadedImage<'static>> {
        struct Search {
            address: u64,
            required: u32,
            forbidden: u32,
            image: Option<LoadedImage<'static>>,
        }

        unsafe extern "C" fn visit(info: *mut PhdrInfo, size: usize, data: *mut c_void) -> i32 {
            // SAFETY: find passes the address of its local request as context.
            let search = unsafe { &mut *data.cast::<Search>() };
            // SAFETY: the LSB callback record is live. The kernel retains the
            // static executable, and the loader retains its published maps.
            search.image = unsafe { LoadedImage::from_info(info, size) }.filter(|image| {
                image
                    .containing_load(search.address, search.required, search.forbidden)
                    .is_some()
            });
            i32::from(search.image.is_some())
        }

        let mut search = Search {
            address,
            required,
            forbidden,
            image: None,
        };
        // SAFETY: the callback is synchronous and search lives through the call.
        unsafe { dl_iterate_phdr(visit, ptr::from_mut(&mut search).cast()) };
        search.image
    }
}

#[cfg(target_os = "breenix")]
impl Memory for LoadedObjects {
    fn read(&self, address: u64, size: usize) -> Option<&[u8]> {
        Self::find(address, PF_R, PF_W)?.read_mapped(address, size)
    }

    fn read_indirect(&self, address: u64, size: usize, endian: Endian) -> Option<u64> {
        Self::find(address, PF_R, 0)?.read_indirect(address, size, endian)
    }
}

#[cfg(target_os = "breenix")]
impl FrameSource for LoadedObjects {
    fn sections_for_pc(&self, pc: u64) -> Option<Sections<'_>> {
        Self::find(pc, PF_X, 0)?.sections()
    }
}

#[cfg(test)]
impl ProgramHeader {
    pub(crate) fn fixture(kind: u32, flags: u32, address: u64, size: u64) -> Self {
        Self {
            kind,
            flags,
            offset: 0,
            virtual_address: address,
            physical_address: 0,
            file_size: size,
            memory_size: size,
            alignment: 1,
        }
    }
}

#[cfg(test)]
impl PhdrInfo {
    pub(crate) fn fixture(bias: u64, headers: &[ProgramHeader], text: u64, got: u64) -> Self {
        Self {
            prefix: PhdrPrefix {
                load_bias: bias,
                name: c"".as_ptr(),
                headers: headers.as_ptr(),
                header_count: headers.len().try_into().unwrap(),
            },
            lsb_fields: [0; 4],
            bases: [bias + text, bias + got],
        }
    }
}
