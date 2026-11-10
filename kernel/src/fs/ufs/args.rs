//! Pure validation of the native-fd mount contract in docs/fs/ufs.md.

pub fn require_root(euid: u32) -> Result<(), u64> {
    if euid == 0 {
        Ok(())
    } else {
        Err(1)
    } // EPERM
}

pub fn parse(euid: u32, fstype: &[u8], flags: u64, data: &[u8]) -> Result<i32, u64> {
    require_root(euid)?;
    if fstype != b"ufs" || flags != 0 {
        return Err(22); // EINVAL
    }
    let bytes: [u8; 4] = data.try_into().map_err(|_| 22u64)?;
    i32::try_from(u32::from_ne_bytes(bytes)).map_err(|_| 9) // EBADF
}
