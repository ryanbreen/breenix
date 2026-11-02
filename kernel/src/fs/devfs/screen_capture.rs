//! Append-only suite capture to the explicitly marked ARM QEMU disk.
pub use screen_capture::REQUEST_MAGIC;

#[cfg(not(target_arch = "aarch64"))]
pub fn write(_request: &[u8]) -> Result<usize,i32> { Err(19) }

#[cfg(target_arch = "aarch64")]
pub fn write(request: &[u8]) -> Result<usize,i32> {
    use crate::block::{virtio::VirtioBlockWrapper, BlockDevice};
    use screen_capture::{CaptureLog, DISK_MAGIC, MAX_RECORD};
    static LOG: spin::Mutex<CaptureLog> = spin::Mutex::new(CaptureLog::new());
    if crate::drivers::fw_cfg::read_string("opt/breenix/screen-capture").as_deref() != Some("on") {
        return Err(19);
    }
    if request.len() != 24 { return Err(22); }
    let address = u64::from_le_bytes(request[8..16].try_into().unwrap());
    let len = u64::from_le_bytes(request[16..24].try_into().unwrap());
    if len == 0 || len > MAX_RECORD as u64 { return Err(22); }
    let mut bytes = alloc::vec::Vec::new();
    bytes.try_reserve_exact(len as usize).map_err(|_| 12)?;
    bytes.resize(len as usize,0);
    crate::syscall::userptr::read_user_bytes(bytes.as_mut_ptr(),address,bytes.len()).map_err(|e| e as i32)?;
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
    log.append(&bytes,disk.num_blocks()*512,|offset,data| {
        for (i,sector) in data.chunks_exact(512).enumerate() {
            disk.write_block(offset/512+i as u64,sector).map_err(|_| "capture-write")?;
        }
        disk.flush().map_err(|_| "capture-flush")
    }).map_err(|_| 5)?;
    Ok(request.len())
}
