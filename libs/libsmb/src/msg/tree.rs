use super::*;

/// MS-SMB2 2.2.10. Preserve the raw mask, including flags unknown to this client.
pub mod share_flags {
    pub const MANUAL_CACHING: u32 = 0;
    pub const AUTO_CACHING: u32 = 0x10;
    pub const VDO_CACHING: u32 = 0x20;
    pub const NO_CACHING: u32 = 0x30;
    pub const DFS: u32 = 1;
    pub const DFS_ROOT: u32 = 2;
    pub const RESTRICT_EXCLUSIVE_OPENS: u32 = 0x100;
    pub const FORCE_SHARED_DELETE: u32 = 0x200;
    pub const ALLOW_NAMESPACE_CACHING: u32 = 0x400;
    pub const ACCESS_BASED_DIRECTORY_ENUM: u32 = 0x800;
    pub const FORCE_LEVELII_OPLOCK: u32 = 0x1000;
    pub const ENABLE_HASH_V1: u32 = 0x2000;
    pub const ENABLE_HASH_V2: u32 = 0x4000;
    pub const ENCRYPT_DATA: u32 = 0x8000;
    pub const IDENTITY_REMOTING: u32 = 0x40000;
    pub const COMPRESS_DATA: u32 = 0x100000;
    pub const ISOLATED_TRANSPORT: u32 = 0x200000;
}

/// MS-SMB2 2.2.9, without the optional remoted-identity extension.
/// Set flags to 0 for SMB 3.0.2; SMB 3.1.1 permits reconnect/redirect bits.
pub struct TreeConnectRequest<'a> {
    pub flags: u16,
    pub path: &'a str,
}
impl TreeConnectRequest<'_> {
    pub fn encode(&self, header: &SyncHeader) -> Result<Vec<u8>, Error> {
        if self.flags & !3 != 0 {
            return Err(Error);
        }
        let path = utf16(self.path)?;
        let mut b = request(header, 3, 8)?;
        put16(&mut b, 64, 9);
        put16(&mut b, 66, self.flags);
        put16(&mut b, 68, 72);
        put16(&mut b, 70, len16(path.len())?);
        b.extend_from_slice(&path);
        Ok(b)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeConnectResponse {
    pub header: SyncHeader,
    pub share_type: u8,
    pub share_flags: u32,
    pub capabilities: u32,
    pub maximal_access: u32,
}
impl TreeConnectResponse {
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        let header = success(b, 3, 16, 16)?;
        if !(1..=3).contains(&b[66]) {
            return Err(Error);
        }
        Ok(Self {
            header,
            share_type: b[66],
            share_flags: u32_at(b, 68)?,
            capabilities: u32_at(b, 72)?,
            maximal_access: u32_at(b, 76)?,
        })
    }
}
/// MS-SMB2 2.2.11.
pub fn encode_tree_disconnect_request(header: &SyncHeader) -> Result<Vec<u8>, Error> {
    empty_request(header, 4)
}
/// MS-SMB2 2.2.12.
pub fn decode_tree_disconnect_response(b: &[u8]) -> Result<SyncHeader, Error> {
    empty_response(b, 4)
}
