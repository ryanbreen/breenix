use crate::{Error, ErrorKind};

const ELF_HEADER_SIZE: u64 = 64;
const PROGRAM_HEADER_SIZE: u64 = 56;
pub(crate) const PT_LOAD: u32 = 1;
pub(crate) const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
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
    pub bias: u64,
    pub shared: bool,
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
            bias,
            shared: kind == 3,
        };
        for segment in image.segments() {
            let segment = segment?;
            match segment.kind {
                PT_LOAD => {
                    if segment.file_size > segment.mem_size
                        || segment
                            .address
                            .checked_add(bias)
                            .and_then(|start| start.checked_add(segment.mem_size))
                            .is_none()
                    {
                        return Err(
                            image.at(segment.header_offset, ErrorKind::InvalidProgramHeader)
                        );
                    }
                    file.slice(segment.offset, segment.file_size)?;
                }
                PT_INTERP => image.shared = false,
                _ => {}
            }
        }
        Ok(image)
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
                })
            })
    }

    /// Translate an ELF virtual address through a readable, file-backed PT_LOAD.
    pub fn file_at(self, address: u64, size: u64, source: u64) -> Result<Bytes<'a>, Error<'a>> {
        for segment in self.segments() {
            let segment = segment?;
            if segment.kind == PT_LOAD && segment.flags & PF_R != 0 {
                if let Some(delta) = segment.contains(address, size, segment.file_size) {
                    return self.file.slice(segment.offset + delta, size);
                }
            }
        }
        Err(self.at(source, ErrorKind::Bounds))
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

    pub fn at(self, offset: u64, kind: ErrorKind) -> Error<'a> {
        Error {
            offset,
            ..self.file.error(kind)
        }
    }
}
