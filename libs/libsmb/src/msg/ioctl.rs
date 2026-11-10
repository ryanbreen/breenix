use super::*;

pub const FSCTL_VALIDATE_NEGOTIATE_INFO: u32 = 0x0014_0204;

/// MS-SMB2 2.2.31.4. Use on SMB 3.0/3.0.2; 3.1.1 uses preauth integrity.
pub struct ValidateNegotiateRequest<'a> {
    pub capabilities: u32,
    pub guid: [u8; 16],
    pub security_mode: u16,
    pub dialects: &'a [u16],
}
impl ValidateNegotiateRequest<'_> {
    /// Constructs the IOCTL envelope per MS-SMB2 3.2.5.5. The caller signs it.
    pub fn encode(&self, header: &SyncHeader) -> Result<Vec<u8>, Error> {
        if self.dialects.is_empty() {
            return Err(Error);
        }
        let count = len16(self.dialects.len())?;
        let mut b = request(header, 11, 56)?;
        put16(&mut b, 64, 57);
        put32(&mut b, 68, FSCTL_VALIDATE_NEGOTIATE_INFO);
        FileId::NONE.put(&mut b, 72);
        put32(&mut b, 88, 120);
        put32(&mut b, 92, 24 + u32::from(count) * 2);
        put32(&mut b, 100, 120);
        put32(&mut b, 108, 24);
        put32(&mut b, 112, 1);
        b.extend_from_slice(&self.capabilities.to_le_bytes());
        b.extend_from_slice(&self.guid);
        b.extend_from_slice(&self.security_mode.to_le_bytes());
        b.extend_from_slice(&count.to_le_bytes());
        for d in self.dialects {
            b.extend_from_slice(&d.to_le_bytes());
        }
        Ok(b)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateNegotiateResponse {
    pub header: SyncHeader,
    pub capabilities: u32,
    pub guid: [u8; 16],
    pub security_mode: u16,
    pub dialect: u16,
}
impl ValidateNegotiateResponse {
    /// MS-SMB2 2.2.32/2.2.32.6. Values must be compared to negotiation by the caller.
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let header = success(b, 11, 49, 48)?;
        if u32_at(b, 68)? != FSCTL_VALIDATE_NEGOTIATE_INFO {
            return Err(Error);
        }
        buffer(b, u32_at(b, 88)? as usize, u32_at(b, 92)? as usize, 112)?;
        let offset = u32_at(b, 96)? as usize;
        let length = u32_at(b, 100)? as usize;
        if length != 24 || offset % 8 != 0 {
            return Err(Error);
        }
        let data = buffer(b, offset, length, 112)?;
        Ok(Self {
            header,
            capabilities: u32_at(data, 0)?,
            guid: data[4..20].try_into().map_err(|_| Error)?,
            security_mode: u16_at(data, 20)?,
            dialect: u16_at(data, 22)?,
        })
    }
}
