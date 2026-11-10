//! SMB2 wire encoding and decoding, independent of transport I/O.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialectCap {
    Default,
    Smb302,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiateContext {
    Preauth { algorithms: Vec<u16>, salt: Vec<u8> },
    Encryption { ciphers: Vec<u16> },
    Unknown { kind: u16, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncHeader {
    pub credit_charge: u16,
    pub status: u32,
    pub command: u16,
    pub credits: u16,
    pub flags: u32,
    pub next_command: u32,
    pub message_id: u64,
    pub tree_id: u32,
    pub session_id: u64,
    pub signature: [u8; 16],
}
impl SyncHeader {
    pub fn encode(&self) -> [u8; 64] {
        let mut b = [0; 64];
        b[..4].copy_from_slice(b"\xfeSMB");
        b[4..6].copy_from_slice(&64u16.to_le_bytes());
        b[6..8].copy_from_slice(&self.credit_charge.to_le_bytes());
        b[8..12].copy_from_slice(&self.status.to_le_bytes());
        b[12..14].copy_from_slice(&self.command.to_le_bytes());
        b[14..16].copy_from_slice(&self.credits.to_le_bytes());
        b[16..20].copy_from_slice(&self.flags.to_le_bytes());
        b[20..24].copy_from_slice(&self.next_command.to_le_bytes());
        b[24..32].copy_from_slice(&self.message_id.to_le_bytes());
        b[36..40].copy_from_slice(&self.tree_id.to_le_bytes());
        b[40..48].copy_from_slice(&self.session_id.to_le_bytes());
        b[48..64].copy_from_slice(&self.signature);
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        if b.len() < 64 || &b[..4] != b"\xfeSMB" || u16_at(b, 4)? != 64 || u32_at(b, 16)? & 2 != 0 {
            return Err(Error);
        }
        Ok(Self {
            credit_charge: u16_at(b, 6)?,
            status: u32_at(b, 8)?,
            command: u16_at(b, 12)?,
            credits: u16_at(b, 14)?,
            flags: u32_at(b, 16)?,
            next_command: u32_at(b, 20)?,
            message_id: u64_at(b, 24)?,
            tree_id: u32_at(b, 36)?,
            session_id: u64_at(b, 40)?,
            signature: b[48..64].try_into().map_err(|_| Error)?,
        })
    }
}
fn u16_at(b: &[u8], n: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        b.get(n..n + 2)
            .ok_or(Error)?
            .try_into()
            .map_err(|_| Error)?,
    ))
}
fn u32_at(b: &[u8], n: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        b.get(n..n + 4)
            .ok_or(Error)?
            .try_into()
            .map_err(|_| Error)?,
    ))
}
fn u64_at(b: &[u8], n: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        b.get(n..n + 8)
            .ok_or(Error)?
            .try_into()
            .map_err(|_| Error)?,
    ))
}

/// Direct TCP framing (MS-SMB2 section 2.1).
pub fn frame(message: &[u8]) -> Result<Vec<u8>, Error> {
    if message.len() > 0xff_ffff {
        return Err(Error);
    }
    let mut result = Vec::with_capacity(4 + message.len());
    result.extend_from_slice(&(message.len() as u32).to_be_bytes());
    result.extend_from_slice(message);
    Ok(result)
}
pub fn unframe(frame: &[u8]) -> Result<&[u8], Error> {
    let header: [u8; 4] = frame.get(..4).ok_or(Error)?.try_into().map_err(|_| Error)?;
    if header[0] != 0 {
        return Err(Error);
    }
    let length = u32::from_be_bytes(header) as usize;
    if frame.len() != length + 4 {
        return Err(Error);
    }
    Ok(&frame[4..])
}

pub fn encode_negotiate_request(cap: DialectCap, guid: [u8; 16], salt: [u8; 32]) -> Vec<u8> {
    let header = SyncHeader {
        credit_charge: 0,
        status: 0,
        command: 0,
        credits: 1,
        flags: 0,
        next_command: 0,
        message_id: 0,
        tree_id: 0,
        session_id: 0,
        signature: [0; 16],
    };
    let mut b = header.encode().to_vec();
    b.extend_from_slice(&36u16.to_le_bytes());
    b.extend_from_slice(&(if cap == DialectCap::Default { 2u16 } else { 1 }).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&0x40u32.to_le_bytes());
    b.extend_from_slice(&guid);
    if cap == DialectCap::Default {
        b.extend_from_slice(&[104, 0, 0, 0, 2, 0, 0, 0]);
    } else {
        b.extend_from_slice(&[0; 8]);
    }
    b.extend_from_slice(&0x0302u16.to_le_bytes());
    if cap == DialectCap::Default {
        b.extend_from_slice(&0x0311u16.to_le_bytes());
        b.extend_from_slice(&[1, 0, 38, 0, 0, 0, 0, 0, 1, 0, 32, 0, 1, 0]);
        b.extend_from_slice(&salt);
        b.extend_from_slice(&[0, 0, 2, 0, 6, 0, 0, 0, 0, 0, 2, 0, 2, 0, 1, 0]);
    }
    b
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiateResponse {
    pub header: SyncHeader,
    pub security_mode: u16,
    pub dialect: u16,
    pub server_guid: [u8; 16],
    pub capabilities: u32,
    pub max_transact_size: u32,
    pub max_read_size: u32,
    pub max_write_size: u32,
    pub security_buffer: Vec<u8>,
    pub contexts: Vec<NegotiateContext>,
}
pub fn decode_negotiate_response(b: &[u8]) -> Result<NegotiateResponse, Error> {
    let header = SyncHeader::decode(b)?;
    if header.command != 0
        || header.flags & 1 == 0
        || header.status != 0
        || header.next_command != 0
        || u16_at(b, 64)? != 65
    {
        return Err(Error);
    }
    let dialect = u16_at(b, 68)?;
    let count = if dialect == 0x0311 {
        u16_at(b, 70)? as usize
    } else {
        0
    };
    let security_offset = u16_at(b, 120)? as usize;
    let security_len = u16_at(b, 122)? as usize;
    let security_buffer = if security_len == 0 {
        Vec::new()
    } else {
        b.get(security_offset..security_offset.checked_add(security_len).ok_or(Error)?)
            .ok_or(Error)?
            .to_vec()
    };
    let mut pos = u32_at(b, 124)? as usize;
    let mut contexts = Vec::new();
    if count > 0 && (pos < 128 || pos % 8 != 0) {
        return Err(Error);
    }
    for i in 0..count {
        let kind = u16_at(b, pos)?;
        let len = u16_at(b, pos + 2)? as usize;
        let start = pos.checked_add(8).ok_or(Error)?;
        let end = start.checked_add(len).ok_or(Error)?;
        let data = b.get(start..end).ok_or(Error)?;
        let context = match kind {
            1 => {
                let n = u16_at(data, 0)? as usize;
                let salt_len = u16_at(data, 2)? as usize;
                let split = 4 + n * 2;
                if n == 0 || split + salt_len != data.len() {
                    return Err(Error);
                }
                let algorithms = (0..n)
                    .map(|i| u16_at(data, 4 + i * 2))
                    .collect::<Result<Vec<_>, _>>()?;
                NegotiateContext::Preauth {
                    algorithms,
                    salt: data[split..].to_vec(),
                }
            }
            2 => {
                let n = u16_at(data, 0)? as usize;
                if n == 0 || 2 + n * 2 != data.len() {
                    return Err(Error);
                }
                let ciphers = (0..n)
                    .map(|i| u16_at(data, 2 + i * 2))
                    .collect::<Result<Vec<_>, _>>()?;
                NegotiateContext::Encryption { ciphers }
            }
            _ => NegotiateContext::Unknown {
                kind,
                data: data.to_vec(),
            },
        };
        contexts.push(context);
        if i + 1 < count {
            pos = end.checked_add(7).ok_or(Error)? & !7;
        }
    }
    Ok(NegotiateResponse {
        header,
        security_mode: u16_at(b, 66)?,
        dialect,
        server_guid: b.get(72..88).ok_or(Error)?.try_into().map_err(|_| Error)?,
        capabilities: u32_at(b, 88)?,
        max_transact_size: u32_at(b, 92)?,
        max_read_size: u32_at(b, 96)?,
        max_write_size: u32_at(b, 100)?,
        security_buffer,
        contexts,
    })
}
