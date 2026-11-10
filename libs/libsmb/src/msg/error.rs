use super::*;

/// MS-ERREF 2.3. Keep unknown status codes intact for the caller's error path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NtStatus(pub u32);
impl NtStatus {
    pub const SUCCESS: Self = Self(0x0000_0000);
    pub const PENDING: Self = Self(0x0000_0103);
    pub const BUFFER_OVERFLOW: Self = Self(0x8000_0005);
    pub const NO_MORE_FILES: Self = Self(0x8000_0006);
    pub const INVALID_HANDLE: Self = Self(0xc000_0008);
    pub const INVALID_PARAMETER: Self = Self(0xc000_000d);
    pub const END_OF_FILE: Self = Self(0xc000_0011);
    pub const MORE_PROCESSING_REQUIRED: Self = Self(0xc000_0016);
    pub const ACCESS_DENIED: Self = Self(0xc000_0022);
    pub const BUFFER_TOO_SMALL: Self = Self(0xc000_0023);
    pub const OBJECT_NAME_NOT_FOUND: Self = Self(0xc000_0034);
    pub const OBJECT_NAME_COLLISION: Self = Self(0xc000_0035);
    pub const OBJECT_PATH_NOT_FOUND: Self = Self(0xc000_003a);
    pub const SHARING_VIOLATION: Self = Self(0xc000_0043);
    pub const DELETE_PENDING: Self = Self(0xc000_0056);
    pub const WRONG_PASSWORD: Self = Self(0xc000_006a);
    pub const LOGON_FAILURE: Self = Self(0xc000_006d);
    pub const PASSWORD_EXPIRED: Self = Self(0xc000_0071);
    pub const ACCOUNT_DISABLED: Self = Self(0xc000_0072);
    pub const DISK_FULL: Self = Self(0xc000_007f);
    pub const INSUFFICIENT_RESOURCES: Self = Self(0xc000_009a);
    pub const MEDIA_WRITE_PROTECTED: Self = Self(0xc000_00a2);
    pub const IO_TIMEOUT: Self = Self(0xc000_00b5);
    pub const FILE_IS_A_DIRECTORY: Self = Self(0xc000_00ba);
    pub const NOT_SUPPORTED: Self = Self(0xc000_00bb);
    pub const NETWORK_NAME_DELETED: Self = Self(0xc000_00c9);
    pub const BAD_NETWORK_NAME: Self = Self(0xc000_00cc);
    pub const DIRECTORY_NOT_EMPTY: Self = Self(0xc000_0101);
    pub const NOT_A_DIRECTORY: Self = Self(0xc000_0103);
    pub const CANCELLED: Self = Self(0xc000_0120);
    pub const CANNOT_DELETE: Self = Self(0xc000_0121);
    pub const FILE_CLOSED: Self = Self(0xc000_0128);
    pub const USER_SESSION_DELETED: Self = Self(0xc000_0203);
    pub const NETWORK_SESSION_EXPIRED: Self = Self(0xc000_035c);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorContext {
    pub id: u32,
    pub data: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorResponse {
    pub header: SyncHeader,
    pub status: NtStatus,
    pub error_data: Vec<u8>,
    pub contexts: Vec<ErrorContext>,
}
impl ErrorResponse {
    /// MS-SMB2 2.2.2/2.2.2.1. Context count zero also covers SMB 3.0.2.
    /// Status-specific payloads (for example symlinks) stay opaque.
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let command = u16_at(b, 12)?;
        let header = response(b, command, 9, 8)?;
        let status = NtStatus(header.status);
        if status == NtStatus::SUCCESS {
            return Err(Error);
        }
        let data = buffer(b, 72, u32_at(b, 68)? as usize, 72)?;
        let mut contexts = Vec::new();
        let mut at = 0;
        for i in 0..b[66] {
            let len = u32_at(data, at)? as usize;
            let id = u32_at(data, at + 4)?;
            let start = at.checked_add(8).ok_or(Error)?;
            let end = start.checked_add(len).ok_or(Error)?;
            contexts.push(ErrorContext {
                id,
                data: data.get(start..end).ok_or(Error)?.to_vec(),
            });
            at = if i + 1 < b[66] { align8(end)? } else { end };
        }
        if b[66] != 0 && at != data.len() {
            return Err(Error);
        }
        Ok(Self {
            header,
            status,
            error_data: data.to_vec(),
            contexts,
        })
    }
}
