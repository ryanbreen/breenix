use super::*;
use crate::u64_at;

/// Credit units for an SMB 2.1 or later multi-credit request.
pub fn credit_charge(len: u32) -> u32 {
    1 + len.saturating_sub(1) / 65536
}
/// Split a transfer into requests no larger than the negotiated maximum.
pub fn plan_chunks(mut len: u64, maximum: u32) -> Result<Vec<u32>, Error> {
    if maximum == 0 {
        return Err(Error);
    }
    let mut chunks = Vec::new();
    while len != 0 {
        let n = len.min(u64::from(maximum)) as u32;
        chunks.push(n);
        len -= u64::from(n);
    }
    Ok(chunks)
}
pub struct ReadRequest {
    pub file_id: FileId,
    pub offset: u64,
    pub length: u32,
}
impl ReadRequest {
    pub fn encode(&self, h: &SyncHeader) -> Result<Vec<u8>, Error> {
        let mut b = request(h, 8, 49)?;
        put16(&mut b, 64, 49);
        put32(&mut b, 68, self.length);
        put64(&mut b, 72, self.offset);
        self.file_id.put(&mut b, 80);
        Ok(b)
    }
}
pub struct WriteRequest<'a> {
    pub file_id: FileId,
    pub offset: u64,
    pub data: &'a [u8],
}
impl WriteRequest<'_> {
    pub fn encode(&self, h: &SyncHeader) -> Result<Vec<u8>, Error> {
        let mut b = request(h, 9, 48)?;
        put16(&mut b, 64, 49);
        put16(&mut b, 66, 112);
        put32(&mut b, 68, len32(self.data.len())?);
        put64(&mut b, 72, self.offset);
        self.file_id.put(&mut b, 80);
        b.extend_from_slice(self.data);
        Ok(b)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct ReadResponse {
    pub header: SyncHeader,
    pub data: Vec<u8>,
    pub remaining: u32,
}
impl ReadResponse {
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let header = success(b, 8, 17, 16)?;
        Ok(Self {
            header,
            data: buffer(b, b[66] as usize, u32_at(b, 68)? as usize, 80)?.to_vec(),
            remaining: u32_at(b, 72)?,
        })
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct WriteResponse {
    pub header: SyncHeader,
    pub count: u32,
    pub remaining: u32,
}
impl WriteResponse {
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let header = success(b, 9, 17, 16)?;
        Ok(Self {
            header,
            count: u32_at(b, 68)?,
            remaining: u32_at(b, 72)?,
        })
    }
}
pub fn encode_flush_request(h: &SyncHeader, file_id: FileId) -> Result<Vec<u8>, Error> {
    let mut b = request(h, 7, 24)?;
    put16(&mut b, 64, 24);
    file_id.put(&mut b, 72);
    Ok(b)
}
pub fn decode_flush_response(b: &[u8]) -> Result<SyncHeader, Error> {
    success(b, 7, 4, 4)
}

pub struct QueryDirectoryRequest<'a> {
    pub file_id: FileId,
    pub pattern: &'a str,
    pub output_length: u32,
    pub flags: u8,
}
impl QueryDirectoryRequest<'_> {
    pub fn encode(&self, h: &SyncHeader) -> Result<Vec<u8>, Error> {
        let name = utf16(self.pattern)?;
        let mut b = request(h, 14, 32)?;
        put16(&mut b, 64, 33);
        b[66] = 37;
        b[67] = self.flags;
        self.file_id.put(&mut b, 72);
        put16(&mut b, 88, 96);
        put16(&mut b, 90, len16(name.len())?);
        put32(&mut b, 92, self.output_length);
        b.extend_from_slice(&name);
        Ok(b)
    }
}
fn decode_name(b: &[u8]) -> Result<String, Error> {
    if b.len() % 2 != 0 {
        return Err(Error);
    }
    let units = b
        .chunks_exact(2)
        .map(|x| u16::from_le_bytes([x[0], x[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&units).map_err(|_| Error)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub file_id: u64,
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub change_time: u64,
    pub end_of_file: u64,
    pub allocation_size: u64,
    pub attributes: u32,
}
pub fn decode_directory_entries(b: &[u8]) -> Result<Vec<DirectoryEntry>, Error> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < b.len() {
        let rest = &b[pos..];
        let next = u32_at(rest, 0)? as usize;
        let end = if next == 0 {
            rest.len()
        } else {
            if next % 8 != 0 || next < 104 || next >= rest.len() {
                return Err(Error);
            }
            next
        };
        let e = rest.get(..end).ok_or(Error)?;
        let n = u32_at(e, 60)? as usize;
        let name = decode_name(
            e.get(104..104usize.checked_add(n).ok_or(Error)?)
                .ok_or(Error)?,
        )?;
        out.push(DirectoryEntry {
            name,
            file_id: u64_at(e, 96)?,
            creation_time: u64_at(e, 8)?,
            last_access_time: u64_at(e, 16)?,
            last_write_time: u64_at(e, 24)?,
            change_time: u64_at(e, 32)?,
            end_of_file: u64_at(e, 40)?,
            allocation_size: u64_at(e, 48)?,
            attributes: u32_at(e, 56)?,
        });
        if next == 0 {
            break;
        }
        pos = pos.checked_add(next).ok_or(Error)?;
    }
    Ok(out)
}
fn output_buffer(b: &[u8], command: u16) -> Result<(SyncHeader, &[u8]), Error> {
    let h = success(b, command, 9, 8)?;
    Ok((
        h,
        buffer(b, u16_at(b, 66)? as usize, u32_at(b, 68)? as usize, 72)?,
    ))
}
#[derive(Debug, PartialEq, Eq)]
pub struct QueryDirectoryResponse {
    pub header: SyncHeader,
    pub entries: Vec<DirectoryEntry>,
}
impl QueryDirectoryResponse {
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let (header, data) = output_buffer(b, 14)?;
        Ok(Self {
            header,
            entries: decode_directory_entries(data)?,
        })
    }
}
pub struct QueryInfoRequest {
    pub file_id: FileId,
    pub output_length: u32,
}
impl QueryInfoRequest {
    pub fn encode(&self, h: &SyncHeader) -> Result<Vec<u8>, Error> {
        let mut b = request(h, 16, 40)?;
        put16(&mut b, 64, 41);
        b[66] = 1;
        b[67] = 18;
        put32(&mut b, 68, self.output_length);
        self.file_id.put(&mut b, 88);
        Ok(b)
    }
}
/// MS-SMB2 QUERY_INFO for MS-FSCC FileFsFullSizeInformation.
pub struct QueryFsSizeRequest {
    pub file_id: FileId,
}
impl QueryFsSizeRequest {
    pub fn encode(&self, h: &SyncHeader) -> Result<Vec<u8>, Error> {
        let mut b = request(h, 16, 40)?;
        put16(&mut b, 64, 41);
        b[66] = 2;
        b[67] = 7;
        put32(&mut b, 68, 32);
        self.file_id.put(&mut b, 88);
        Ok(b)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFsFullSizeInformation {
    pub total_allocation_units: u64,
    pub caller_available_allocation_units: u64,
    pub actual_available_allocation_units: u64,
    pub sectors_per_allocation_unit: u32,
    pub bytes_per_sector: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub struct QueryFsSizeResponse {
    pub header: SyncHeader,
    pub info: FileFsFullSizeInformation,
}
impl QueryFsSizeResponse {
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let (header, data) = output_buffer(b, 16)?;
        Ok(Self {
            header,
            info: FileFsFullSizeInformation {
                total_allocation_units: u64_at(data, 0)?,
                caller_available_allocation_units: u64_at(data, 8)?,
                actual_available_allocation_units: u64_at(data, 16)?,
                sectors_per_allocation_unit: u32_at(data, 24)?,
                bytes_per_sector: u32_at(data, 28)?,
            },
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAllInformation {
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub change_time: u64,
    pub attributes: u32,
    pub allocation_size: u64,
    pub end_of_file: u64,
    pub links: u32,
    pub delete_pending: bool,
    pub directory: bool,
    pub index_number: u64,
    pub ea_size: u32,
    pub access: u32,
    pub position: u64,
    pub mode: u32,
    pub alignment: u32,
    pub name: String,
}
pub fn decode_file_all_information(b: &[u8]) -> Result<FileAllInformation, Error> {
    let n = u32_at(b, 96)? as usize;
    Ok(FileAllInformation {
        creation_time: u64_at(b, 0)?,
        last_access_time: u64_at(b, 8)?,
        last_write_time: u64_at(b, 16)?,
        change_time: u64_at(b, 24)?,
        attributes: u32_at(b, 32)?,
        allocation_size: u64_at(b, 40)?,
        end_of_file: u64_at(b, 48)?,
        links: u32_at(b, 56)?,
        delete_pending: *b.get(60).ok_or(Error)? != 0,
        directory: *b.get(61).ok_or(Error)? != 0,
        index_number: u64_at(b, 64)?,
        ea_size: u32_at(b, 72)?,
        access: u32_at(b, 76)?,
        position: u64_at(b, 80)?,
        mode: u32_at(b, 88)?,
        alignment: u32_at(b, 92)?,
        name: decode_name(
            b.get(100..100usize.checked_add(n).ok_or(Error)?)
                .ok_or(Error)?,
        )?,
    })
}
#[derive(Debug, PartialEq, Eq)]
pub struct QueryInfoResponse {
    pub header: SyncHeader,
    pub info: FileAllInformation,
}
impl QueryInfoResponse {
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let (header, data) = output_buffer(b, 16)?;
        Ok(Self {
            header,
            info: decode_file_all_information(data)?,
        })
    }
}
pub enum SetInfo<'a> {
    EndOfFile(u64),
    Basic {
        creation_time: u64,
        last_access_time: u64,
        last_write_time: u64,
        change_time: u64,
        attributes: u32,
    },
    Rename {
        replace_if_exists: bool,
        name: &'a str,
    },
    Disposition(bool),
}
pub struct SetInfoRequest<'a> {
    pub file_id: FileId,
    pub info: SetInfo<'a>,
}
impl SetInfoRequest<'_> {
    pub fn encode(&self, h: &SyncHeader) -> Result<Vec<u8>, Error> {
        let (class, data) = match &self.info {
            SetInfo::EndOfFile(n) => (20, n.to_le_bytes().to_vec()),
            SetInfo::Basic {
                creation_time,
                last_access_time,
                last_write_time,
                change_time,
                attributes,
            } => {
                let mut v = vec![0; 40];
                put64(&mut v, 0, *creation_time);
                put64(&mut v, 8, *last_access_time);
                put64(&mut v, 16, *last_write_time);
                put64(&mut v, 24, *change_time);
                put32(&mut v, 32, *attributes);
                (4, v)
            }
            SetInfo::Rename {
                replace_if_exists,
                name,
            } => {
                let name = utf16(name)?;
                let mut v = vec![0; 20];
                v[0] = u8::from(*replace_if_exists);
                put32(&mut v, 16, len32(name.len())?);
                v.extend_from_slice(&name);
                if v.len() < 24 {
                    v.resize(24, 0);
                }
                (10, v)
            }
            SetInfo::Disposition(v) => (13, vec![u8::from(*v)]),
        };
        let mut b = request(h, 17, 32)?;
        put16(&mut b, 64, 33);
        b[66] = 1;
        b[67] = class;
        put32(&mut b, 68, len32(data.len())?);
        put16(&mut b, 72, 96);
        self.file_id.put(&mut b, 80);
        b.extend_from_slice(&data);
        Ok(b)
    }
}
pub fn decode_set_info_response(b: &[u8]) -> Result<SyncHeader, Error> {
    success(b, 17, 2, 2)
}
