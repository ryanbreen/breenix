//! The write fence's fault-injection exercise (docs/metal/README.md, "The write fence"), compiled only into a
//! fault-test kernel (feature `block_fence_inject`). After the boot's mounts it reads the sector just below the root
//! partition and the one just above it and writes each back unchanged through the raw disk device, which bypasses the
//! partition device's own bounds; the fence must refuse both. Then it writes the partition's second sector (ext2
//! leaves its first 1024 bytes unused) back unchanged, which the fence must allow. A write the fence wrongly allowed
//! would store the bytes the sector already held.

pub fn run() {
    let fence = super::fence();
    let (Some(disk), Some(&(first, last))) = (fence.disk, fence.ranges().first()) else {
        log::warn!("[fence-fault] FAIL: no fence was set");
        return;
    };
    let Some(raw) = super::discovered().into_iter().find(|device| device.hardware_disk() == Some(disk)) else {
        log::warn!("[fence-fault] FAIL: the fenced disk is not attached");
        return;
    };
    let probe = |lba: u64| -> Result<bool, &'static str> {
        let mut sector = [0u8; super::gpt::SECTOR];
        raw.read_block(lba, &mut sector).map_err(|_| "a probe sector is unreadable")?;
        Ok(raw.write_block(lba, &sector).is_ok())
    };
    let word = |written: bool| if written { "written" } else { "refused" };
    match (first.checked_sub(1).map(probe), probe(last + 1), probe(first + 1)) {
        (Some(Ok(below)), Ok(above), Ok(inside)) => {
            if !below && !above && inside {
                log::info!("[fence-fault] PASS: below=refused above=refused inside=written");
            } else {
                log::warn!("[fence-fault] FAIL: below={} above={} inside={}", word(below), word(above), word(inside));
            }
        }
        _ => log::warn!("[fence-fault] FAIL: the probe sectors could not be read"),
    }
}
