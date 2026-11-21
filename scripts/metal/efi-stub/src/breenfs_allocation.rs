//! Discover the published allocation set from the BreenFS superblocks and space tree.
//! The independent host checker reads data blocks in the reconstructed image to validate their CRCs.

#[path = "breenfs_crc32c.rs"]
mod crc32c;

pub const BLOCK: usize = 4096;
const NODE_BLOCKS: usize = 4;
const NODE: usize = NODE_BLOCKS * BLOCK;
const HEIGHT: usize = 8;
pub const SCRATCH: usize = HEIGHT * NODE;
const MAX_DATA: usize = 32 << 30;

fn number(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap())
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

fn mark(map: &mut [u8], first: usize, count: usize) {
    for block in first..first + count {
        map[block / 8] |= 1 << (block % 8);
    }
}

struct Tree<'a> {
    total: usize,
    uuid: [u8; 16],
    map: &'a mut [u8],
    visited: &'a mut [u8],
}

impl Tree<'_> {
    fn walk(
        &mut self,
        pointer: [u8; 24],
        upper_generation: u64,
        scratch: &mut [u8],
        read: &mut (impl FnMut(usize, &mut [u8]) -> Result<(), ()> + ?Sized),
    ) -> Result<(), ()> {
        let first = usize::try_from(number(&pointer[..8])).map_err(|_| ())?;
        let generation = number(&pointer[8..16]);
        let level = pointer[20];
        if first < 20
            || first > self.total - 4 - NODE_BLOCKS
            || level as usize >= HEIGHT
            || generation == 0
            || generation > upper_generation
        {
            return Err(());
        }
        if (first..first + NODE_BLOCKS).any(|i| self.visited[i / 8] & (1 << (i % 8)) != 0) {
            return Err(());
        }
        mark(self.visited, first, NODE_BLOCKS);
        let (bytes, children) = scratch.split_at_mut(NODE);
        read(first, bytes)?;
        if &bytes[..8] != b"BREENFSN"
            || bytes[12] != 2
            || bytes[13] != level
            || bytes[16..32] != self.uuid
            || number(&bytes[32..40]) != first as u64
            || number(&bytes[40..48]) != generation
            || word(&bytes[8..12]) != word(&pointer[16..20])
            || word(&bytes[8..12]) != crc32c::checksum(&bytes[12..])
        {
            return Err(());
        }
        mark(self.map, first, NODE_BLOCKS);
        let count = u16::from_le_bytes(bytes[14..16].try_into().unwrap()) as usize;
        let stride = if level == 0 { 21 } else { 37 };
        if count > (bytes.len() - 48) / stride {
            return Err(());
        }
        for record in bytes[48..].chunks_exact(stride).take(count) {
            if level > 0 {
                let mut child = [0; 24];
                child[..20].copy_from_slice(&record[17..37]);
                child[20] = level - 1;
                self.walk(child, generation, children, read)?;
            } else {
                match record[8] {
                    kind @ (0x80 | 0x83) => {
                        let start = usize::try_from(number(&record[..8])).map_err(|_| ())?;
                        let length = usize::try_from(number(&record[9..17])).map_err(|_| ())?;
                        if start < 20
                            || start >= self.total - 4
                            || length == 0
                            || length > (self.total - 4 - start).min(256)
                            || (kind == 0x83 && length != NODE_BLOCKS)
                        {
                            return Err(());
                        }
                        mark(self.map, start, length);
                    }
                    // FREE, PENDING and ROOT contain no further allocation records.
                    0x81 | 0x82 | 0x90 => {}
                    _ => return Err(()),
                }
            }
        }
        Ok(())
    }
}

pub fn discover(
    total: usize,
    map: &mut [u8],
    visited: &mut [u8],
    scratch: &mut [u8],
    mut read: impl FnMut(usize, &mut [u8]) -> Result<(), ()>,
) -> Result<usize, ()> {
    if total < 28
        || map.len() != total.div_ceil(8)
        || visited.len() != map.len()
        || scratch.len() < SCRATCH
    {
        return Err(());
    }
    let mut latest = None;
    let mut uuid = None;
    for (ring, first) in [16, total - 4].into_iter().enumerate() {
        mark(map, first, 4);
        for slot in 0..4 {
            let bytes = &mut scratch[..BLOCK];
            read(first + slot, bytes)?;
            let generation = number(&bytes[32..40]);
            if &bytes[..8] != b"BREENFSS"
                || word(&bytes[8..12]) != crc32c::checksum(&bytes[12..])
                || word(&bytes[12..16]) != 1
                || generation == 0
                || bytes[40] as usize != ring
                || bytes[41] as usize != slot
                || generation % 4 != slot as u64
                || word(&bytes[44..48]) != BLOCK as u32
                || word(&bytes[48..52]) != NODE as u32
                || number(&bytes[56..64]) != total as u64
                || number(&bytes[80..88]) != 1
            {
                continue;
            }
            let found = bytes[16..32].try_into().unwrap();
            if uuid.is_some_and(|uuid| uuid != found) {
                return Err(());
            }
            uuid = Some(found);
            let pointer = bytes[88..112].try_into().unwrap();
            if latest.is_some_and(|(n, previous)| generation == n && pointer != previous) {
                return Err(());
            }
            if latest.is_none_or(|(n, _)| generation > n) {
                latest = Some((generation, pointer));
            }
        }
    }
    let (generation, pointer) = latest.ok_or(())?;
    Tree {
        total,
        uuid: uuid.unwrap(),
        map,
        visited,
    }
    .walk(pointer, generation, scratch, &mut read)?;
    let count: usize = map.iter().map(|value| value.count_ones() as usize).sum();
    if count > MAX_DATA / BLOCK {
        return Err(());
    }
    Ok(count)
}
