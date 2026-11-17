//! Type detection only; recovery validates superblocks and their roots.
use crate::{block::BlockDevice, fs::errno::Errno, syscall::errno::EIO};

pub fn probe(device: &dyn BlockDevice) -> Result<bool, Errno> {
    const BLOCK: usize = 4096;
    let sector = device.block_size();
    if sector < 8 || sector > BLOCK || BLOCK % sector != 0 {
        return Ok(false);
    }
    let sectors_per_block = (BLOCK / sector) as u64;
    let total = device.num_blocks() / sectors_per_block;
    if total < 24 {
        return Ok(false);
    }
    let mut bytes = [0; BLOCK];
    // Either ring can be the sole survivor. Only its first sector holds magic.
    for start in [16, total - 4] {
        for slot in 0..4 {
            device
                .read_block((start + slot) * sectors_per_block, &mut bytes[..sector])
                .map_err(|_| Errno(EIO))?;
            if bytes.starts_with(b"BREENFSS") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
