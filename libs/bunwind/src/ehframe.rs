//! LSB Core 5.0, Generic Part, §§10.5.1 and 10.6: exception frames and their index.
//! <https://refspecs.linuxfoundation.org/LSB_5.0.0/LSB-Core-generic/LSB-Core-generic/ehframechpt.html>
//!
//! Addresses are runtime virtual addresses. Instruction streams borrow the input
//! bytes and include any DW_CFA_nop padding. The reader neither executes CFI nor
//! calls personality routines. It validates the fields visited by the lookup.

use core::ops::Range;

use crate::cfi::{self, read_sleb128, read_uleb128, Endian};

/// A contiguous section and its runtime address.
#[derive(Clone, Copy)]
pub struct Section<'a> {
    pub address: u64,
    pub bytes: &'a [u8],
}

/// Image data needed to find and decode a frame description entry.
#[derive(Clone, Copy)]
pub struct Sections<'a> {
    pub frame: Section<'a>,
    pub header: Option<Section<'a>>,
    /// The image's text base for text-relative pointers in `.eh_frame`.
    pub text_base: Option<u64>,
    /// The image's GOT base for data-relative pointers in `.eh_frame`.
    pub data_base: Option<u64>,
    /// Target pointer width in bytes (4 or 8).
    pub address_size: u8,
    pub endian: Endian,
}

/// Reads the target pointer slot for a DW_EH_PE_indirect reference.
pub trait Memory {
    /// Return the mapped bytes at `address` if the entire span is readable.
    fn read(&self, address: u64, size: usize) -> Option<&[u8]>;
}

/// Supplies the sections for the image containing a PC. The guest source can
/// locate the header through PT_GNU_EH_FRAME (LSB Core 5.0, Table 11-1).
pub trait FrameSource: Memory {
    fn sections_for_pc(&self, pc: u64) -> Option<Sections<'_>>;
}

/// Find an FDE using a caller-provided image discovery source.
pub fn find_fde(source: &(impl FrameSource + ?Sized), pc: u64) -> Result<Option<Fde<'_>>, Error> {
    match source.sections_for_pc(pc) {
        Some(sections) => sections.find_fde(source, pc),
        None => Ok(None),
    }
}

/// Decode the index's .eh_frame pointer before the loader knows its bounds.
#[cfg(target_os = "breenix")]
pub(crate) fn frame_address(
    header: Section<'_>,
    memory: &impl Memory,
    address_size: usize,
    endian: Endian,
) -> Result<u64, Error> {
    let mut reader = Reader::new(header, SectionKind::Header, endian);
    Ok(header_prefix(&mut reader, memory, address_size)?.frame_address)
}

impl<'a> Sections<'a> {
    /// Return the FDE whose half-open PC range covers `pc`.
    ///
    /// Fixed-width tables are binary searched by initial location. LEB128
    /// tables are traversed in order because entries have no fixed stride.
    /// If the index is omitted, the frame records are scanned directly.
    pub fn find_fde(
        &self,
        memory: &(impl Memory + ?Sized),
        pc: u64,
    ) -> Result<Option<Fde<'a>>, Error> {
        if !matches!(self.address_size, 4 | 8) {
            return Err(self.frame_reader().error(ErrorKind::InvalidAddressSize));
        }
        if let Some(header) = self.header {
            let reader = Reader::new(header, SectionKind::Header, self.endian);
            if let Some(table) = self.table(reader, memory)? {
                let Some(entry) = table.find(self, memory, pc)? else {
                    return Ok(None);
                };
                let offset = entry
                    .fde
                    .checked_sub(self.frame.address)
                    .and_then(|offset| usize::try_from(offset).ok())
                    .ok_or(entry.location.error(ErrorKind::InvalidRecord))?;
                let record = self
                    .frame_record(offset, self.frame.bytes.len())?
                    .ok_or(entry.location.error(ErrorKind::InvalidRecord))?;
                let fde = self.fde(record, memory)?;
                if fde.pc_range.start != entry.start {
                    return Err(entry.location.error(ErrorKind::InvalidTable));
                }
                return Ok(fde.pc_range.contains(&pc).then_some(fde));
            }
        }

        let mut offset = 0;
        while offset < self.frame.bytes.len() {
            let Some(record) = self.frame_record(offset, self.frame.bytes.len())? else {
                break;
            };
            offset = record.body.offset + record.body.bytes.len();
            if record.id != 0 {
                let fde = self.fde(record, memory)?;
                if fde.pc_range.contains(&pc) {
                    return Ok(Some(fde));
                }
            }
        }
        Ok(None)
    }

    fn frame_reader(&self) -> Reader<'a> {
        Reader::new(self.frame, SectionKind::Frame, self.endian)
    }

    fn frame_record(&self, start: usize, limit: usize) -> Result<Option<Record<'a>>, Error> {
        let mut reader = self.frame_reader();
        reader.bytes = reader.bytes.get(start..limit).ok_or(Error {
            section: SectionKind::Frame,
            offset: start,
            kind: ErrorKind::Truncated,
        })?;
        reader.offset = start;
        let length = match reader.uint(4)? {
            0 => return Ok(None),
            0xffff_ffff => reader.uint(8)?,
            length => length,
        };
        let length = usize::try_from(length).map_err(|_| reader.error(ErrorKind::Overflow))?;
        let mut body = reader.take(length)?;
        // The CIE ID / FDE backlink is four bytes even with an extended length.
        let id_field = body;
        let id = body.uint(4)? as u32;
        Ok(Some(Record {
            start,
            id_field,
            id,
            body,
        }))
    }

    fn table<'h>(
        &self,
        mut reader: Reader<'h>,
        memory: &(impl Memory + ?Sized),
    ) -> Result<Option<Table<'h>>, Error> {
        let prefix = header_prefix(&mut reader, memory, self.address_size.into())?;
        if prefix.frame_address != self.frame.address {
            return Err(prefix.frame_field.error(ErrorKind::InvalidHeader));
        }

        let Some(count_encoding) = prefix.count_encoding else {
            return Ok(None);
        };
        if count_encoding.base != Base::Absolute || count_encoding.indirect {
            return Err(prefix.count_field.error(ErrorKind::InvalidHeader));
        }
        let count_field = reader;
        let count = count_encoding.number(&mut reader, self.address_size.into())?;
        let count =
            usize::try_from(count).map_err(|_| count_field.error(ErrorKind::InvalidTable))?;
        let Some(encoding) = prefix.table_encoding else {
            return Ok(None);
        };
        let entry_size = encoding
            .width(self.address_size.into())
            .map(|width| width * 2);
        if count > reader.bytes.len() / entry_size.unwrap_or(2) {
            return Err(reader.error(ErrorKind::Truncated));
        }
        Ok(Some(Table {
            reader,
            count,
            encoding,
            entry_size,
        }))
    }

    fn cie(
        &self,
        record: Record<'a>,
        memory: &(impl Memory + ?Sized),
    ) -> Result<CieEncoding<'a>, Error> {
        let mut reader = record.body;
        let version_field = reader;
        let version = reader.byte()?;
        if version != 1 {
            return Err(version_field.error(ErrorKind::UnsupportedVersion(version)));
        }
        let augmentation_field = reader;
        let augmentation = reader.c_string()?;
        let code_alignment = reader.uleb()?;
        let data_alignment = reader.sleb()?;
        let return_address_register = u64::from(reader.byte()?);
        let has_augmentation_data = augmentation.first() == Some(&b'z');
        let mut fde_encoding = Encoding::ABSOLUTE_POINTER;
        let mut lsda_encoding = None;
        let mut personality = None;
        let mut signal_frame = false;
        let mut data = if has_augmentation_data {
            reader.block()?
        } else {
            reader.take(0)?
        };
        let first_flag = usize::from(has_augmentation_data);
        for (index, &flag) in augmentation.iter().enumerate().skip(first_flag) {
            let flag_field = Reader {
                offset: augmentation_field.offset + index,
                ..augmentation_field
            };
            if augmentation[..index].contains(&flag) {
                return Err(flag_field.error(ErrorKind::InvalidAugmentation));
            }
            match flag {
                b'R' if has_augmentation_data => {
                    let encoding_field = data;
                    fde_encoding = Encoding::read(&mut data)?
                        .ok_or(encoding_field.error(ErrorKind::InvalidAugmentation))?;
                }
                b'P' if has_augmentation_data => {
                    let encoding = Encoding::read(&mut data)?;
                    personality = self.optional_pointer(&mut data, encoding, memory)?;
                }
                b'L' if has_augmentation_data => lsda_encoding = Encoding::read(&mut data)?,
                b'S' => signal_frame = true,
                _ => return Err(flag_field.error(ErrorKind::UnsupportedAugmentation(flag))),
            }
        }
        if !data.bytes.is_empty() {
            return Err(data.error(ErrorKind::InvalidAugmentation));
        }
        Ok(CieEncoding {
            cie: Cie {
                code_alignment,
                data_alignment,
                return_address_register,
                initial_instructions: reader.bytes,
                personality,
                signal_frame,
            },
            has_augmentation_data,
            fde_encoding,
            lsda_encoding,
        })
    }

    fn fde(&self, record: Record<'a>, memory: &(impl Memory + ?Sized)) -> Result<Fde<'a>, Error> {
        let invalid_cie = record.id_field.error(ErrorKind::InvalidCiePointer);
        let cie_start = record
            .id_field
            .offset
            .checked_sub(record.id as usize)
            .ok_or(invalid_cie)?;
        if record.id == 0 || cie_start >= record.start {
            return Err(invalid_cie);
        }
        let cie_record = self
            .frame_record(cie_start, record.start)?
            .ok_or(invalid_cie)?;
        if cie_record.id != 0 {
            return Err(invalid_cie);
        }
        let cie = self.cie(cie_record, memory)?;
        let mut reader = record.body;
        let start = self.pointer(&mut reader, cie.fde_encoding, memory, false)?;
        let range_field = reader;
        // A range is a byte count: keep only R's value format.
        let size = cie
            .fde_encoding
            .number(&mut reader, self.address_size.into())?;
        let end = u64::try_from(size)
            .ok()
            .and_then(|size| start.checked_add(size))
            .ok_or(range_field.error(ErrorKind::InvalidRange))?;
        let mut lsda = None;
        if cie.has_augmentation_data {
            let mut data = reader.block()?;
            lsda = self.optional_pointer(&mut data, cie.lsda_encoding, memory)?;
            if !data.bytes.is_empty() {
                return Err(data.error(ErrorKind::InvalidAugmentation));
            }
        }
        Ok(Fde {
            pc_range: start..end,
            cie: cie.cie,
            instructions: reader.bytes,
            lsda,
        })
    }

    fn optional_pointer(
        &self,
        reader: &mut Reader<'_>,
        encoding: Option<Encoding>,
        memory: &(impl Memory + ?Sized),
    ) -> Result<Option<u64>, Error> {
        let Some(encoding) = encoding else {
            return Ok(None);
        };
        let pointer = self.pointer(reader, encoding, memory, true)?;
        Ok((pointer != 0).then_some(pointer))
    }

    fn pointer(
        &self,
        reader: &mut Reader<'_>,
        encoding: Encoding,
        memory: &(impl Memory + ?Sized),
        nullable: bool,
    ) -> Result<u64, Error> {
        decode_pointer(
            reader,
            encoding,
            memory,
            nullable,
            self.address_size.into(),
            self.text_base,
            self.data_base,
        )
    }
}

struct HeaderPrefix<'a> {
    frame_address: u64,
    frame_field: Reader<'a>,
    count_field: Reader<'a>,
    count_encoding: Option<Encoding>,
    table_encoding: Option<Encoding>,
}

fn header_prefix<'a>(
    reader: &mut Reader<'a>,
    memory: &(impl Memory + ?Sized),
    address_size: usize,
) -> Result<HeaderPrefix<'a>, Error> {
    let version_field = *reader;
    let version = reader.byte()?;
    if version != 1 {
        return Err(version_field.error(ErrorKind::UnsupportedVersion(version)));
    }
    let encoding_field = *reader;
    let frame_encoding =
        Encoding::read(reader)?.ok_or(encoding_field.error(ErrorKind::InvalidHeader))?;
    let count_field = *reader;
    let count_encoding = Encoding::read(reader)?;
    let table_encoding = Encoding::read(reader)?;
    let frame_field = *reader;
    let frame_address = decode_pointer(
        reader,
        frame_encoding,
        memory,
        false,
        address_size,
        None,
        None,
    )?;
    Ok(HeaderPrefix {
        frame_address,
        frame_field,
        count_field,
        count_encoding,
        table_encoding,
    })
}

fn decode_pointer(
    reader: &mut Reader<'_>,
    encoding: Encoding,
    memory: &(impl Memory + ?Sized),
    nullable: bool,
    width: usize,
    text_base: Option<u64>,
    data_base: Option<u64>,
) -> Result<u64, Error> {
    let field = *reader;
    let value = encoding.number(reader, width)?;
    if nullable && value == 0 {
        return Ok(0);
    }
    let base = match encoding.base {
        Base::Absolute => 0,
        Base::Pc => field
            .address
            .checked_add(field.offset as u64)
            .ok_or(field.error(ErrorKind::Overflow))?,
        Base::Text => text_base.ok_or(field.error(ErrorKind::MissingTextBase))?,
        Base::Data => match field.section {
            SectionKind::Header => field.address,
            SectionKind::Frame => data_base.ok_or(field.error(ErrorKind::MissingDataBase))?,
        },
    };
    let address =
        u64::try_from(i128::from(base) + value).map_err(|_| field.error(ErrorKind::Overflow))?;
    if !encoding.indirect {
        return Ok(address);
    }
    let error = field.error(ErrorKind::UnreadablePointer(address));
    let mut bytes = memory.read(address, width).ok_or(error)?;
    cfi::uint(&mut bytes, width, field.endian).map_err(|_| error)
}

/// Fields shared by the frame description entries referencing this CIE.
#[derive(Debug)]
pub struct Cie<'a> {
    pub code_alignment: u64,
    pub data_alignment: i64,
    pub return_address_register: u64,
    pub initial_instructions: &'a [u8],
    pub personality: Option<u64>,
    pub signal_frame: bool,
}

/// The matching FDE with its CIE and optional language-specific data address.
#[derive(Debug)]
pub struct Fde<'a> {
    pub pc_range: Range<u64>,
    pub cie: Cie<'a>,
    pub instructions: &'a [u8],
    pub lsda: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SectionKind {
    Frame,
    Header,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Truncated,
    Overflow,
    InvalidAddressSize,
    UnsupportedVersion(u8),
    UnsupportedEncoding(u8),
    UnsupportedAugmentation(u8),
    InvalidAugmentation,
    InvalidHeader,
    InvalidTable,
    InvalidRecord,
    InvalidCiePointer,
    InvalidRange,
    MissingTextBase,
    MissingDataBase,
    UnreadablePointer(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub section: SectionKind,
    /// Byte offset of the offending field from the start of its section.
    pub offset: usize,
    pub kind: ErrorKind,
}

struct Record<'a> {
    start: usize,
    id_field: Reader<'a>,
    id: u32,
    body: Reader<'a>,
}

struct CieEncoding<'a> {
    cie: Cie<'a>,
    has_augmentation_data: bool,
    fde_encoding: Encoding,
    lsda_encoding: Option<Encoding>,
}

struct Table<'a> {
    reader: Reader<'a>,
    count: usize,
    encoding: Encoding,
    entry_size: Option<usize>,
}

struct TableEntry<'a> {
    start: u64,
    fde: u64,
    location: Reader<'a>,
}

impl<'a> Table<'a> {
    fn find(
        &self,
        sections: &Sections<'_>,
        memory: &(impl Memory + ?Sized),
        pc: u64,
    ) -> Result<Option<TableEntry<'a>>, Error> {
        let Some(entry_size) = self.entry_size else {
            let mut reader = self.reader;
            let mut candidate = None;
            let mut previous = None;
            for _ in 0..self.count {
                let location = reader;
                let start = sections.pointer(&mut reader, self.encoding, memory, false)?;
                let fde = sections.pointer(&mut reader, self.encoding, memory, false)?;
                if previous.is_some_and(|previous| previous > start) {
                    return Err(location.error(ErrorKind::InvalidTable));
                }
                if start <= pc {
                    candidate = Some(TableEntry {
                        start,
                        fde,
                        location,
                    });
                }
                previous = Some(start);
            }
            return Ok(candidate);
        };

        let mut low = 0;
        let mut high = self.count;
        // Find the first entry after pc, then use its predecessor.
        while low < high {
            let mid = low + (high - low) / 2;
            let mut reader = self.reader;
            reader.take(mid * entry_size)?;
            let start = sections.pointer(&mut reader, self.encoding, memory, false)?;
            if start <= pc {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low == 0 {
            return Ok(None);
        }
        let mut reader = self.reader;
        reader.take((low - 1) * entry_size)?;
        let location = reader;
        let start = sections.pointer(&mut reader, self.encoding, memory, false)?;
        let fde = sections.pointer(&mut reader, self.encoding, memory, false)?;
        Ok(Some(TableEntry {
            start,
            fde,
            location,
        }))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Base {
    Absolute,
    Pc,
    Text,
    Data,
}

#[derive(Clone, Copy)]
enum Format {
    Pointer,
    Uleb,
    Sleb,
    Unsigned(usize),
    Signed(usize),
}

#[derive(Clone, Copy)]
struct Encoding {
    format: Format,
    base: Base,
    indirect: bool,
}

impl Encoding {
    const ABSOLUTE_POINTER: Self = Self {
        format: Format::Pointer,
        base: Base::Absolute,
        indirect: false,
    };

    fn read(reader: &mut Reader<'_>) -> Result<Option<Self>, Error> {
        let field = *reader;
        let byte = reader.byte()?;
        if byte == 0xff {
            return Ok(None);
        }
        let error = field.error(ErrorKind::UnsupportedEncoding(byte));
        let format = match byte & 0x0f {
            0x00 => Format::Pointer,
            0x01 => Format::Uleb,
            0x02 => Format::Unsigned(2),
            0x03 => Format::Unsigned(4),
            0x04 => Format::Unsigned(8),
            0x09 => Format::Sleb,
            0x0a => Format::Signed(2),
            0x0b => Format::Signed(4),
            0x0c => Format::Signed(8),
            _ => return Err(error),
        };
        let base = match byte & 0x70 {
            0x00 => Base::Absolute,
            0x10 => Base::Pc,
            0x20 => Base::Text,
            0x30 => Base::Data,
            _ => return Err(error),
        };
        Ok(Some(Self {
            format,
            base,
            indirect: byte & 0x80 != 0,
        }))
    }

    fn width(self, pointer_width: usize) -> Option<usize> {
        match self.format {
            Format::Pointer => Some(pointer_width),
            Format::Signed(width) | Format::Unsigned(width) => Some(width),
            Format::Uleb | Format::Sleb => None,
        }
    }

    fn number(self, reader: &mut Reader<'_>, pointer_width: usize) -> Result<i128, Error> {
        // i128 holds both the entire unsigned 64-bit domain and signed displacements.
        match self.format {
            Format::Pointer => Ok(reader.uint(pointer_width)?.into()),
            Format::Uleb => Ok(reader.uleb()?.into()),
            Format::Sleb => Ok(reader.sleb()?.into()),
            Format::Unsigned(width) => Ok(reader.uint(width)?.into()),
            Format::Signed(width) => {
                let shift = 64 - width * 8;
                let bits = reader.uint(width)? << shift;
                Ok(((bits as i64) >> shift).into())
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    address: u64,
    endian: Endian,
    section: SectionKind,
}

impl<'a> Reader<'a> {
    fn new(section: Section<'a>, kind: SectionKind, endian: Endian) -> Self {
        Self {
            bytes: section.bytes,
            offset: 0,
            address: section.address,
            endian,
            section: kind,
        }
    }

    fn error(&self, kind: ErrorKind) -> Error {
        Error {
            section: self.section,
            offset: self.offset,
            kind,
        }
    }

    fn take(&mut self, size: usize) -> Result<Self, Error> {
        let bytes = self
            .bytes
            .get(..size)
            .ok_or(self.error(ErrorKind::Truncated))?;
        let result = Self { bytes, ..*self };
        self.bytes = &self.bytes[size..];
        self.offset += size;
        Ok(result)
    }

    fn read<T>(
        &mut self,
        read: impl FnOnce(&mut &'a [u8]) -> Result<T, cfi::ErrorKind>,
    ) -> Result<T, Error> {
        let len = self.bytes.len();
        let value = read(&mut self.bytes).map_err(|kind| {
            self.error(match kind {
                cfi::ErrorKind::IntegerOverflow => ErrorKind::Overflow,
                _ => ErrorKind::Truncated,
            })
        })?;
        self.offset += len - self.bytes.len();
        Ok(value)
    }

    fn uint(&mut self, width: usize) -> Result<u64, Error> {
        let endian = self.endian;
        self.read(|bytes| cfi::uint(bytes, width, endian))
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.uint(1)? as u8)
    }

    fn uleb(&mut self) -> Result<u64, Error> {
        self.read(read_uleb128)
    }

    fn sleb(&mut self) -> Result<i64, Error> {
        self.read(read_sleb128)
    }

    fn c_string(&mut self) -> Result<&'a [u8], Error> {
        let len = self
            .bytes
            .iter()
            .position(|&byte| byte == 0)
            .ok_or(self.error(ErrorKind::Truncated))?;
        Ok(&self.take(len + 1)?.bytes[..len])
    }

    fn block(&mut self) -> Result<Self, Error> {
        let field = *self;
        let size = usize::try_from(self.uleb()?).map_err(|_| field.error(ErrorKind::Overflow))?;
        self.take(size)
    }
}

#[cfg(test)]
include!("ehframe_test.rs");
