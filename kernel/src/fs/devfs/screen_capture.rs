//! Append-only suite capture to the explicitly marked ARM QEMU disk.
pub use screen_capture::REQUEST_MAGIC;

#[cfg(not(target_arch = "aarch64"))]
pub fn write(_request: &[u8]) -> Result<usize,i32> { Err(19) }

#[cfg(target_arch = "aarch64")]
pub fn write(request: &[u8]) -> Result<usize,i32> {
    use crate::block::{virtio::VirtioBlockWrapper, BlockDevice};
    use screen_capture::{CaptureLog, DISK_MAGIC, MAX_RECORD};
    static LOG: spin::Mutex<CaptureLog> = spin::Mutex::new(CaptureLog::new());
    if !crate::platform_config::is_qemu()
        || crate::drivers::fw_cfg::read_string("opt/breenix/screen-capture").as_deref() != Some("on") {
        return Err(19);
    }
    if request.len() != 24 { return Err(22); }
    let address = u64::from_le_bytes(request[8..16].try_into().unwrap());
    let len = u64::from_le_bytes(request[16..24].try_into().unwrap());
    if len < 512 || len > MAX_RECORD as u64 || len%512 != 0 { return Err(22); }
    let mut disk = None;
    for index in 0..crate::drivers::virtio::block_mmio::MAX_BLOCK_DEVICES {
        if let Some(candidate) = VirtioBlockWrapper::new(index) {
            let mut marker = [0;512];
            if candidate.read_block(0,&mut marker).is_ok() && &marker[..8] == DISK_MAGIC {
                if disk.is_some() { return Err(22); }
                disk = Some(candidate);
            }
        }
    }
    let disk = disk.ok_or(19)?;
    let mut log = LOG.try_lock().ok_or(16)?;
    log.append_stream(len as usize,disk.num_blocks()*512,|offset,buf| {
        let source = address.checked_add(offset as u64).ok_or("capture-address")?;
        crate::syscall::userptr::read_user_bytes(buf.as_mut_ptr(),source,buf.len())
            .map_err(|_| "capture-user-read")
    },|offset,data| disk.write_block(offset/512,data).map_err(|_| "capture-write"),
        || disk.flush().map_err(|_| "capture-flush")).map_err(|_| 5)?;
    Ok(request.len())
}
