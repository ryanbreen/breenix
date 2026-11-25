use crate::{Error, ErrorKind};

const ELF_HEADER_SIZE: u64 = 64;
const PROGRAM_HEADER_SIZE: u64 = 56;
pub(crate) const PT_LOAD: u32 = 1;
pub(crate) const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
pub(crate) const PT_TLS: u32 = 7;
pub(crate) const PT_GNU_RELRO: u32 = 0x6474_e552;
pub(crate) const PF_W: u32 = 2;
pub(crate) const PF_R: u32 = 4;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Bytes<'a> {
    pub name: &'a [u8],
    pub data: &'a [u8],
    pub offset: u64,
}

impl<'a> Bytes<'a> {
    pub fn error(self, kind: ErrorKind) -> Error<'a> {
        Error {
            object: self.name,
            offset: self.offset,
            kind,
            detail: b"",
        }
    }

    pub fn slice(self, offset: u64, size: u64) -> Result<Self, Error<'a>> {
        let bounds = || Error {
            offset: self.offset.saturating_add(offset),
            ..self.error(ErrorKind::Bounds)
        };
        let start = usize::try_from(offset).map_err(|_| bounds())?;
        let len = usize::try_from(size).map_err(|_| bounds())?;
        let data = self
            .data
            .get(start..)
            .and_then(|tail| tail.get(..len))
            .ok_or_else(bounds)?;
        Ok(Self {
            name: self.name,
            data,
            offset: self.offset + offset,
        })
    }

    fn read<const N: usize>(self, offset: u64) -> Result<[u8; N], Error<'a>> {
        let bytes = self.slice(offset, N as u64)?;
        bytes
            .data
            .try_into()
            .map_err(|_| bytes.error(ErrorKind::Bounds))
    }

    pub fn u16(self, offset: u64) -> Result<u16, Error<'a>> {
        self.read(offset).map(u16::from_le_bytes)
    }

    pub fn u32(self, offset: u64) -> Result<u32, Error<'a>> {
        self.read(offset).map(u32::from_le_bytes)
    }

    pub fn u64(self, offset: u64) -> Result<u64, Error<'a>> {
        self.read(offset).map(u64::from_le_bytes)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Image<'a> {
    pub file: Bytes<'a>,
    headers: Bytes<'a>,
    mapped: Option<Mapped>,
    pub bias: u64,
    pub shared: bool,
}

pub(crate) type Snapshot = fn(&[u8]) -> Option<&'static [u8]>;

#[derive(Debug, Clone, Copy)]
struct Mapped {
    #[cfg(feature = "interpreter")]
    phdr: u64,
    snapshot: Snapshot,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Segment {
    pub header_offset: u64,
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub address: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
}

impl Segment {
    fn contains(self, address: u64, size: u64, limit: u64) -> Option<u64> {
        let displacement = address.checked_sub(self.address)?;
        let remaining = limit.checked_sub(displacement)?;
        (size <= remaining).then_some(displacement)
    }
}

impl<'a> Image<'a> {
    pub fn parse(file: Bytes<'a>, bias: u64) -> Result<Self, Error<'a>> {
        let header = file.slice(0, ELF_HEADER_SIZE)?;
        let kind = header.u16(16)?;
        if !header.data.starts_with(b"\x7fELF\x02\x01\x01")
            || !matches!(kind, 2 | 3)
            || header.u16(18)? != 62
            || header.u32(20)? != 1
            || u64::from(header.u16(52)?) != ELF_HEADER_SIZE
            || (kind == 2 && bias != 0)
        {
            return Err(header.error(ErrorKind::UnsupportedElf));
        }
        let phoff = header.u64(32)?;
        let count = header.u16(56)?;
        if !matches!(count, 1..=0xfffe) || u64::from(header.u16(54)?) != PROGRAM_HEADER_SIZE {
            return Err(Error {
                offset: phoff,
                ..header.error(ErrorKind::InvalidProgramHeader)
            });
        }
        let headers = file.slice(phoff, u64::from(count) * PROGRAM_HEADER_SIZE)?;
        let mut image = Self {
            file,
            headers,
            mapped: None,
            bias,
            shared: kind == 3,
        };
        image.validate_loads()?;
        Ok(image)
    }

    /// The kernel mapped the PT_LOADs and supplied their bias and PHDR table.
    /// SAFETY: the file-backed PT_LOADs must be mapped at their biased addresses.
    pub unsafe fn from_mapped(
        name: &'a [u8], headers: &[u8], bias: u64, entry: u64, snapshot: Snapshot,
    ) -> Result<Self, Error<'a>> {
        let file = Bytes { name, data: &[], offset: 0 };
        if headers.is_empty() || headers.len() as u64 % PROGRAM_HEADER_SIZE != 0 {
            return Err(file.error(ErrorKind::InvalidProgramHeader));
        }
        let mut image = Self {
            file,
            headers: Bytes {
                data: snapshot(headers).ok_or_else(|| file.error(ErrorKind::Capacity))?,
                ..file
            },
            mapped: Some(Mapped {
                #[cfg(feature = "interpreter")]
                phdr: headers.as_ptr() as u64,
                snapshot,
            }),
            bias,
            shared: false,
        };
        image.validate_loads()?;
        let link_entry = entry.checked_sub(bias)
            .ok_or_else(|| file.error(ErrorKind::InvalidProgramHeader))?;
        image.address(link_entry, 1, 1, 0)?;
        Ok(image)
    }

    fn validate_loads(&mut self) -> Result<(), Error<'a>> {
        for segment in self.segments() {
            let segment = segment?;
            match segment.kind {
                PT_LOAD => {
                    if segment.file_size > segment.mem_size
                        || segment
                            .address
                            .checked_add(self.bias)
                            .and_then(|start| start.checked_add(segment.mem_size))
                            .is_none()
                    {
                        return Err(
                            self.at(segment.header_offset, ErrorKind::InvalidProgramHeader)
                        );
                    }
                    if self.mapped.is_none() {
                        self.file.slice(segment.offset, segment.file_size)?;
                    } else if segment.offset.checked_add(segment.file_size).is_none() {
                        return Err(self.at(segment.header_offset, ErrorKind::InvalidProgramHeader));
                    }
                }
                PT_INTERP => self.shared = false,
                _ => {}
            }
        }
        Ok(())
    }

    pub fn segments(self) -> impl Iterator<Item = Result<Segment, Error<'a>>> {
        let headers = self.headers;
        headers
            .data
            .chunks_exact(PROGRAM_HEADER_SIZE as usize)
            .enumerate()
            .map(move |(i, data)| {
                let header = Bytes {
                    name: headers.name,
                    data,
                    offset: headers.offset + i as u64 * PROGRAM_HEADER_SIZE,
                };
                Ok(Segment {
                    header_offset: header.offset,
                    kind: header.u32(0)?,
                    flags: header.u32(4)?,
                    offset: header.u64(8)?,
                    address: header.u64(16)?,
                    file_size: header.u64(32)?,
                    mem_size: header.u64(40)?,
                    align: header.u64(48)?,
                })
            })
    }

    /// Translate an ELF virtual address through a readable, file-backed PT_LOAD.
    pub fn file_at(self, address: u64, size: u64, source: u64) -> Result<Bytes<'a>, Error<'a>> {
        let offset = self.file_offset(address, size, source)?;
        match self.mapped {
            Some(mapped) => self.snapshot_at(mapped, self.bias + address, offset, size),
            None => self.file.slice(offset, size),
        }
    }

    pub fn file_offset(self, address: u64, size: u64, source: u64) -> Result<u64, Error<'a>> {
        for segment in self.segments() {
            let segment = segment?;
            if segment.kind == PT_LOAD && segment.flags & PF_R != 0 {
                if let Some(delta) = segment.contains(address, size, segment.file_size) {
                    return Ok(segment.offset + delta);
                }
            }
        }
        Err(self.at(source, ErrorKind::Bounds))
    }

    fn snapshot_at(self, mapped: Mapped, address: u64, offset: u64, size: u64) -> Result<Bytes<'a>, Error<'a>> {
        let size = usize::try_from(size).ok().filter(|&len| len <= isize::MAX as usize)
            .ok_or_else(|| self.at(offset, ErrorKind::Bounds))?;
        // SAFETY: the caller of from_mapped supplied the live image. The
        // caller of snapshot_at checked this span against a readable PT_LOAD.
        // Keep a copy because later relocations can write to the load.
        let source = unsafe { core::slice::from_raw_parts(address as *const u8, size) };
        let data = (mapped.snapshot)(source).ok_or_else(|| self.at(offset, ErrorKind::Capacity))?;
        Ok(Bytes { data, offset, ..self.file })
    }

    /// Validate an in-memory span, including the zero-filled PT_LOAD tail.
    pub fn address(
        self,
        address: u64,
        size: u64,
        flags: u32,
        source: u64,
    ) -> Result<u64, Error<'a>> {
        for segment in self.segments() {
            let segment = segment?;
            if segment.kind == PT_LOAD
                && segment.flags & flags == flags
                && segment.contains(address, size, segment.mem_size).is_some()
            {
                // Image::parse validated the biased end of this load.
                return Ok(self.bias + address);
            }
        }
        Err(self.at(source, ErrorKind::Bounds))
    }

    #[cfg(feature = "interpreter")]
    pub fn interpreter_name(self) -> Option<&'a core::ffi::CStr> {
        let header = self
            .segments()
            .filter_map(Result::ok)
            .find(|header| header.kind == PT_INTERP)?;
        let bytes = match self.mapped {
            Some(mapped) => {
                let address = self.runtime_offset(header.offset, header.file_size)?;
                self.snapshot_at(mapped, address, header.offset, header.file_size).ok()?
            }
            None => self.file.slice(header.offset, header.file_size).ok()?,
        };
        core::ffi::CStr::from_bytes_with_nul(bytes.data).ok()
    }

    #[cfg(feature = "interpreter")]
    pub fn runtime_offset(self, offset: u64, size: u64) -> Option<u64> {
        self.segments().filter_map(Result::ok).find_map(|segment| {
            let displacement = offset.checked_sub(segment.offset)?;
            let remaining = segment.file_size.checked_sub(displacement)?;
            (segment.kind == PT_LOAD && segment.flags & PF_R != 0 && size <= remaining)
                .then(|| self.bias + segment.address + displacement)
        })
    }

    #[cfg(feature = "interpreter")]
    pub fn program_headers(self) -> (*const core::ffi::c_void, usize) {
        let size = self.headers.data.len();
        if let Some(mapped) = self.mapped {
            return (mapped.phdr as *const _, size / PROGRAM_HEADER_SIZE as usize);
        }
        let address = self
            .runtime_offset(self.headers.offset, size as u64)
            .map_or(self.headers.data.as_ptr(), |address| address as *const u8);
        (address.cast(), size / PROGRAM_HEADER_SIZE as usize)
    }

    // gABI §3: find a named, allocated section when section headers are present.
    #[cfg(feature = "interpreter")]
    pub fn section_base(self, name: &[u8]) -> Option<u64> {
        let file = self.file;
        let table = file.u64(40).ok()?;
        let count = u64::from(file.u16(60).ok()?);
        let names = u64::from(file.u16(62).ok()?);
        if table == 0 || file.u16(58).ok()? != 64 || names >= count {
            return None;
        }
        let sections = file.slice(table, count * 64).ok()?;
        let strings = sections.slice(names * 64, 64).ok()?;
        let strings = file
            .slice(strings.u64(24).ok()?, strings.u64(32).ok()?)
            .ok()?;
        (0..count).find_map(|index| {
            let section = sections.slice(index * 64, 64).ok()?;
            let flags = section.u64(8).ok()?;
            let offset = section.u32(0).ok()? as usize;
            let tail = strings.data.get(offset..)?;
            if flags & 2 == 0 || !tail.strip_prefix(name)?.starts_with(&[0]) {
                return None;
            }
            let size = section.u64(32).ok()?;
            if size == 0 {
                return None;
            }
            self.address(section.u64(16).ok()?, size, PF_R, section.offset)
                .ok()
        })
    }

    pub fn at(self, offset: u64, kind: ErrorKind) -> Error<'a> {
        Error {
            offset,
            ..self.file.error(kind)
        }
    }
}
