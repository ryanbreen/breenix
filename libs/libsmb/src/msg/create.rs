use super::*;
use crate::u64_at;

/// MS-SMB2 2.2.13.1. File and directory rights share bit positions.
pub mod access_mask {
    pub const READ_DATA: u32 = 0x0001;
    pub const LIST_DIRECTORY: u32 = READ_DATA;
    pub const WRITE_DATA: u32 = 0x0002;
    pub const ADD_FILE: u32 = WRITE_DATA;
    pub const APPEND_DATA: u32 = 0x0004;
    pub const ADD_SUBDIRECTORY: u32 = APPEND_DATA;
    pub const READ_EA: u32 = 0x0008;
    pub const WRITE_EA: u32 = 0x0010;
    pub const EXECUTE: u32 = 0x0020;
    pub const TRAVERSE: u32 = EXECUTE;
    pub const DELETE_CHILD: u32 = 0x0040;
    pub const READ_ATTRIBUTES: u32 = 0x0080;
    pub const WRITE_ATTRIBUTES: u32 = 0x0100;
    pub const DELETE: u32 = 0x0001_0000;
    pub const READ_CONTROL: u32 = 0x0002_0000;
    pub const WRITE_DAC: u32 = 0x0004_0000;
    pub const WRITE_OWNER: u32 = 0x0008_0000;
    pub const SYNCHRONIZE: u32 = 0x0010_0000;
    pub const ACCESS_SYSTEM_SECURITY: u32 = 0x0100_0000;
    pub const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
    pub const GENERIC_ALL: u32 = 0x1000_0000;
    pub const GENERIC_EXECUTE: u32 = 0x2000_0000;
    pub const GENERIC_WRITE: u32 = 0x4000_0000;
    pub const GENERIC_READ: u32 = 0x8000_0000;
}
/// MS-SMB2 2.2.13 CreateOptions.
pub mod create_options {
    pub const DIRECTORY_FILE: u32 = 0x0000_0001;
    pub const WRITE_THROUGH: u32 = 0x0000_0002;
    pub const SEQUENTIAL_ONLY: u32 = 0x0000_0004;
    pub const NO_INTERMEDIATE_BUFFERING: u32 = 0x0000_0008;
    pub const NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    pub const NO_EA_KNOWLEDGE: u32 = 0x0000_0200;
    pub const RANDOM_ACCESS: u32 = 0x0000_0800;
    pub const DELETE_ON_CLOSE: u32 = 0x0000_1000;
    pub const OPEN_FOR_BACKUP_INTENT: u32 = 0x0000_4000;
    pub const NO_COMPRESSION: u32 = 0x0000_8000;
    pub const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    pub const OPEN_NO_RECALL: u32 = 0x0040_0000;
}
pub mod share_access {
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const DELETE: u32 = 4;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CreateDisposition {
    Supersede = 0,
    Open = 1,
    Create = 2,
    OpenIf = 3,
    Overwrite = 4,
    OverwriteIf = 5,
}

/// MS-SMB2 2.2.14.1, opaque server identifiers (not host pointers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId {
    pub persistent: u64,
    pub volatile: u64,
}
impl FileId {
    pub const NONE: Self = Self {
        persistent: u64::MAX,
        volatile: u64::MAX,
    };
    pub(super) fn put(self, b: &mut [u8], at: usize) {
        put64(b, at, self.persistent);
        put64(b, at + 8, self.volatile);
    }
    fn decode(b: &[u8], at: usize) -> Result<Self, Error> {
        Ok(Self {
            persistent: u64_at(b, at)?,
            volatile: u64_at(b, at + 8)?,
        })
    }
}

/// MS-SMB2 2.2.13.2. Context-specific payloads are deliberately opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateContext {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
}
fn encode_contexts(contexts: &[CreateContext]) -> Result<Vec<u8>, Error> {
    let mut b = Vec::new();
    for (i, context) in contexts.iter().enumerate() {
        if context.name.len() < 4 {
            return Err(Error);
        }
        let name_len = len16(context.name.len())?;
        let data_len = len32(context.data.len())?;
        let data_start = align8(16 + context.name.len())?;
        let size = align8(data_start.checked_add(context.data.len()).ok_or(Error)?)?;
        let mut c = vec![0; size];
        if i + 1 < contexts.len() {
            put32(&mut c, 0, len32(size)?);
        }
        put16(&mut c, 4, 16);
        put16(&mut c, 6, name_len);
        if data_len != 0 {
            put16(&mut c, 10, len16(data_start)?);
        }
        put32(&mut c, 12, data_len);
        c[16..16 + context.name.len()].copy_from_slice(&context.name);
        c[data_start..data_start + context.data.len()].copy_from_slice(&context.data);
        b.extend_from_slice(&c);
    }
    len32(b.len())?;
    Ok(b)
}
fn decode_contexts(b: &[u8], offset: usize, length: usize) -> Result<Vec<CreateContext>, Error> {
    if length == 0 {
        if offset != 0 {
            return Err(Error);
        }
        return Ok(Vec::new());
    }
    if offset % 8 != 0 {
        return Err(Error);
    }
    let mut rest = buffer(b, offset, length, 152)?;
    let mut contexts = Vec::new();
    loop {
        let next = u32_at(rest, 0)? as usize;
        let c = if next == 0 {
            rest
        } else {
            if next < 16 || next % 8 != 0 || next >= rest.len() {
                return Err(Error);
            }
            rest.get(..next).ok_or(Error)?
        };
        let name_offset = u16_at(c, 4)? as usize;
        let name_len = u16_at(c, 6)? as usize;
        let data_offset = u16_at(c, 10)? as usize;
        let data_len = u32_at(c, 12)? as usize;
        if name_len < 4 || name_offset % 8 != 0 || (data_len != 0 && data_offset % 8 != 0) {
            return Err(Error);
        }
        let name = buffer(c, name_offset, name_len, 16)?.to_vec();
        // MS-SMB2 2.2.13.2: ignore DataOffset when DataLength == 0.
        let data = if data_len == 0 {
            Vec::new()
        } else {
            buffer(c, data_offset, data_len, 16)?.to_vec()
        };
        contexts.push(CreateContext { name, data });
        if next == 0 {
            break;
        }
        rest = &rest[next..];
    }
    Ok(contexts)
}

pub struct CreateRequest<'a> {
    pub name: &'a str,
    pub oplock_level: u8,
    pub impersonation_level: u32,
    pub desired_access: u32,
    pub file_attributes: u32,
    pub share_access: u32,
    pub disposition: CreateDisposition,
    pub options: u32,
    pub contexts: &'a [CreateContext],
}
impl CreateRequest<'_> {
    /// MS-SMB2 2.2.13. Names are share-relative UTF-16LE without a terminator.
    pub fn encode(&self, header: &SyncHeader) -> Result<Vec<u8>, Error> {
        let name = utf16(self.name)?;
        let contexts = encode_contexts(self.contexts)?;
        let mut b = request(header, 5, 56)?;
        put16(&mut b, 64, 57);
        b[67] = self.oplock_level;
        put32(&mut b, 68, self.impersonation_level);
        put32(&mut b, 88, self.desired_access);
        put32(&mut b, 92, self.file_attributes);
        put32(&mut b, 96, self.share_access);
        put32(&mut b, 100, self.disposition as u32);
        put32(&mut b, 104, self.options);
        put16(&mut b, 108, 120);
        put16(&mut b, 110, len16(name.len())?);
        b.extend_from_slice(&name);
        if !contexts.is_empty() {
            b.resize(align8(b.len())?, 0);
            let offset = len32(b.len())?;
            put32(&mut b, 112, offset);
            put32(&mut b, 116, len32(contexts.len())?);
            b.extend_from_slice(&contexts);
        }
        if b.len() == 120 {
            b.push(0);
        }
        Ok(b)
    }
}

/// Wire FILETIMEs are kept as unsigned values; conversion belongs to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileAttributes {
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub change_time: u64,
    pub allocation_size: u64,
    pub end_of_file: u64,
    pub file_attributes: u32,
}
impl FileAttributes {
    fn decode(b: &[u8], at: usize) -> Result<Self, Error> {
        Ok(Self {
            creation_time: u64_at(b, at)?,
            last_access_time: u64_at(b, at + 8)?,
            last_write_time: u64_at(b, at + 16)?,
            change_time: u64_at(b, at + 24)?,
            allocation_size: u64_at(b, at + 32)?,
            end_of_file: u64_at(b, at + 40)?,
            file_attributes: u32_at(b, at + 48)?,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateResponse {
    pub header: SyncHeader,
    pub oplock_level: u8,
    pub flags: u8,
    pub create_action: u32,
    pub attributes: FileAttributes,
    pub file_id: FileId,
    pub contexts: Vec<CreateContext>,
}
impl CreateResponse {
    /// MS-SMB2 2.2.14.
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let header = success(b, 5, 89, 88)?;
        Ok(Self {
            header,
            oplock_level: b[66],
            flags: b[67],
            create_action: u32_at(b, 68)?,
            attributes: FileAttributes::decode(b, 72)?,
            file_id: FileId::decode(b, 128)?,
            contexts: decode_contexts(b, u32_at(b, 144)? as usize, u32_at(b, 148)? as usize)?,
        })
    }
}

pub struct CloseRequest {
    pub file_id: FileId,
    pub postquery_attributes: bool,
}
impl CloseRequest {
    /// MS-SMB2 2.2.15.
    pub fn encode(&self, header: &SyncHeader) -> Result<Vec<u8>, Error> {
        let mut b = request(header, 6, 24)?;
        put16(&mut b, 64, 24);
        put16(&mut b, 66, u16::from(self.postquery_attributes));
        self.file_id.put(&mut b, 72);
        Ok(b)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseResponse {
    pub header: SyncHeader,
    pub attributes: Option<FileAttributes>,
}
impl CloseResponse {
    /// MS-SMB2 2.2.16. Attributes are absent when POSTQUERY_ATTRIB is clear.
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let header = success(b, 6, 60, 60)?;
        let attributes = if u16_at(b, 66)? & 1 != 0 {
            Some(FileAttributes::decode(b, 72)?)
        } else {
            None
        };
        Ok(Self { header, attributes })
    }
}
