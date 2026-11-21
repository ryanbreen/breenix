//! Read-only allocation discovery for the cold migration export (BreenFS design 2.1–2.5).
use crate::persistent_export::{encode_hex, Partition};
use crate::suite_disk::{self, Buffer, CHUNK};
use metal_efi::{say, sha256::Sha256, uefi::*};

use crate::breenfs_allocation::{self, BLOCK};

pub(super) unsafe fn export(
    boot: &BootServices,
    image: Handle,
    role: &str,
    disk: Partition,
    total: usize,
    url: &[u8],
) -> Result<(), Status> {
    let map_buf = Buffer::new(boot, total.div_ceil(8), 8)?;
    let map = core::slice::from_raw_parts_mut(map_buf.data, total.div_ceil(8));
    map.fill(0);
    let visited_buf = Buffer::new(boot, total.div_ceil(8), 8)?;
    let visited = core::slice::from_raw_parts_mut(visited_buf.data, total.div_ceil(8));
    visited.fill(0);
    let scratch_buf = Buffer::new(boot, breenfs_allocation::SCRATCH, 8)?;
    let scratch = core::slice::from_raw_parts_mut(scratch_buf.data, breenfs_allocation::SCRATCH);
    let node = Buffer::new(boot, 4 * BLOCK, disk.io_align)?;
    let mut reads = 0usize;
    let count = breenfs_allocation::discover(total, map, visited, scratch, |block, output| {
        if let Err(status) = disk.read(block, output.len(), &node) {
            say!("sparse export allocation read failed: {:#x}", status);
            return Err(());
        }
        output.copy_from_slice(core::slice::from_raw_parts(node.data, output.len()));
        reads += 1;
        if reads % 4096 == 0 {
            say!("sparse export allocation reads={}", reads);
        }
        Ok(())
    })
    .map_err(|()| VOLUME_CORRUPTED)?;
    let header = (count as u64).to_le_bytes();
    let prefix = header.iter().chain(map.iter()).copied();
    let size = header.len() + map.len() + count * BLOCK;
    let buffer = Buffer::new(boot, CHUNK, 8)?;
    let data = Buffer::new(boot, BLOCK, disk.io_align)?;
    let mut source = (0..total).filter(|block| map[block / 8] & (1 << (block % 8)) != 0);
    let mut prefix = prefix;
    let mut cursor = BLOCK;
    let mut hash = Sha256::new();
    suite_disk::upload(boot, image, size, &buffer, url, |_, len| {
        let dest = core::slice::from_raw_parts_mut(buffer.data, len);
        let mut offset = 0;
        while offset < len {
            if let Some(byte) = prefix.next() {
                dest[offset] = byte;
                offset += 1;
                continue;
            }
            if cursor == BLOCK {
                disk.read(source.next().ok_or(VOLUME_CORRUPTED)?, BLOCK, &data)?;
                cursor = 0;
            }
            let count = (len - offset).min(BLOCK - cursor);
            let bytes = core::slice::from_raw_parts(data.data.add(cursor), count);
            dest[offset..offset + count].copy_from_slice(bytes);
            offset += count;
            cursor += count;
        }
        hash.update(dest);
        Ok(())
    })?;
    let mut hex = [0; 64];
    encode_hex(&hash.finalize(), &mut hex);
    say!(
        "sparse export {} bytes={} sha256={}",
        role,
        size,
        core::str::from_utf8(&hex).unwrap()
    );
    Ok(())
}
