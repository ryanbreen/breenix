use super::*;

/// MS-SMB2 2.2.6 SessionFlags (distinct from request binding flags).
pub mod session_flags {
    pub const IS_GUEST: u16 = 1;
    pub const IS_NULL: u16 = 2;
    pub const ENCRYPT_DATA: u16 = 4;
}

/// MS-SMB2 2.2.5. Encode the reserved Channel field as 0.
pub struct SessionSetupRequest<'a> {
    pub flags: u8,
    pub security_mode: u8,
    pub capabilities: u32,
    pub previous_session_id: u64,
    pub security_buffer: &'a [u8],
}
impl SessionSetupRequest<'_> {
    pub fn encode(&self, header: &SyncHeader) -> Result<Vec<u8>, Error> {
        let length = len16(self.security_buffer.len())?;
        let mut b = request(header, 1, 24)?;
        put16(&mut b, 64, 25);
        b[66] = self.flags;
        b[67] = self.security_mode;
        put32(&mut b, 68, self.capabilities);
        put16(&mut b, 76, 88);
        put16(&mut b, 78, length);
        put64(&mut b, 80, self.previous_session_id);
        b.extend_from_slice(self.security_buffer);
        Ok(b)
    }
}

/// MS-SMB2 2.2.6. MORE_PROCESSING_REQUIRED carries the next auth token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSetupResponse {
    pub header: SyncHeader,
    pub status: NtStatus,
    pub session_flags: u16,
    pub security_buffer: Vec<u8>,
}
impl SessionSetupResponse {
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let header = response(b, 1, 9, 8)?;
        let status = NtStatus(header.status);
        if status != NtStatus::SUCCESS && status != NtStatus::MORE_PROCESSING_REQUIRED {
            return Err(Error);
        }
        Ok(Self {
            header,
            status,
            session_flags: u16_at(b, 66)?,
            security_buffer: buffer(b, u16_at(b, 68)? as usize, u16_at(b, 70)? as usize, 72)?
                .to_vec(),
        })
    }
}

/// MS-SMB2 2.2.7.
pub fn encode_logoff_request(header: &SyncHeader) -> Result<Vec<u8>, Error> {
    empty_request(header, 2)
}
/// MS-SMB2 2.2.8. Reserved fields are ignored on receipt.
pub fn decode_logoff_response(b: &[u8]) -> Result<SyncHeader, Error> {
    empty_response(b, 2)
}
