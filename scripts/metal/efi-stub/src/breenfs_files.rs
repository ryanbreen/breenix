//! Bounded, read-only access to the default system tree, from the BreenFS v1
//! on-disk format. Every node is checked against its parent pointer and CRC.
//! Files used by the manager must be dense and root-controlled.
#[path = "breenfs_crc32c.rs"]
mod crc32c;
#[path = "breenfs_siphash.rs"]
mod siphash;

pub const BLOCK: usize = 4096;
pub const NODE: usize = 4 * BLOCK;
type Ptr = [u8; 24];
type Key = (u64, u8, u64);

fn short(bytes: &[u8]) -> usize { u16::from_le_bytes(bytes.try_into().unwrap()) as usize }
fn word(bytes: &[u8]) -> u32 { u32::from_le_bytes(bytes.try_into().unwrap()) }
fn wide(bytes: &[u8]) -> u64 { u64::from_le_bytes(bytes.try_into().unwrap()) }
fn key(bytes: &[u8]) -> Key { (wide(&bytes[..8]), bytes[8], wide(&bytes[9..17])) }
fn crc(bytes: &[u8]) -> bool { word(&bytes[8..12]) == crc32c::checksum(&bytes[12..]) }

pub struct Reader<'a, Read> {
    total: u64,
    uuid: [u8; 16],
    generation: u64,
    space: Ptr,
    hash_key: [u8; 16],
    scratch: &'a mut [u8; NODE],
    read: Read,
}

pub struct File {
    tree: Ptr,
    inode: u64,
    pub size: usize,
}

impl<'a, Read: FnMut(u64, &mut [u8]) -> Result<(), ()>> Reader<'a, Read> {
    pub fn open(total: u64, scratch: &'a mut [u8; NODE], mut read: Read) -> Result<Self, ()> {
        if total < 28 { return Err(()); }
        let mut latest = None;
        for (ring, first) in [16, total - 4].into_iter().enumerate() {
            for slot in 0..4 {
                let sb = &mut scratch[..BLOCK];
                if read(first + slot, sb).is_err() { continue; }
                let generation = wide(&sb[32..40]);
                if &sb[..8] != b"BREENFSS" || !crc(sb) || word(&sb[12..16]) != 1
                    || generation == 0 || sb[40] as usize != ring || u64::from(sb[41]) != slot
                    || generation % 4 != slot || word(&sb[44..48]) != BLOCK as u32
                    || word(&sb[48..52]) != NODE as u32 || wide(&sb[56..64]) != total
                { continue; }
                // A valid newer slot with unknown incompatibility must not be
                // bypassed in favor of a potentially retired older tree.
                let found: (u64, [u8; 16], Ptr, [u8; 16], u64) = (generation,
                    sb[16..32].try_into().unwrap(), sb[88..112].try_into().unwrap(),
                    sb[136..152].try_into().unwrap(), wide(&sb[80..88]));
                if let Some(previous) = latest {
                    let (old_generation, old_uuid, _, _, _) = previous;
                    if old_uuid != found.1 || (old_generation == generation && previous != found) {
                        return Err(());
                    }
                    if old_generation >= generation { continue; }
                }
                latest = Some(found);
            }
        }
        let (generation, uuid, space, hash_key, features) = latest.ok_or(())?;
        if features != 1 { return Err(()); }
        Ok(Self { total, uuid, generation, space, hash_key, scratch, read })
    }

    fn item(&mut self, mut ptr: Ptr, tree: u8, target: Key) -> Result<(Key, &[u8]), ()> {
        let mut upper = self.generation;
        loop {
            let address = wide(&ptr[..8]);
            let generation = wide(&ptr[8..16]);
            let level = ptr[20];
            if address < 20 || address > self.total - 8 || generation == 0
                || generation > upper || level >= 8 || ptr[21..] != [0; 3]
            { return Err(()); }
            (self.read)(address, self.scratch)?;
            let node = &*self.scratch;
            if &node[..8] != b"BREENFSN" || !crc(node) || ptr[16..20] != node[8..12]
                || node[12] != tree || node[13] != level || node[16..32] != self.uuid
                || wide(&node[32..40]) != address || wide(&node[40..48]) != generation
            { return Err(()); }
            let stride = if level == 0 { 21 } else { 37 };
            let count = short(&node[14..16]);
            let end = 48 + count * stride;
            let headers = node.get(48..end).ok_or(())?;
            let mut previous = None;
            let mut selected = None;
            for header in headers.chunks_exact(stride) {
                let here = key(header);
                if previous.is_some_and(|old| old >= here) { return Err(()); }
                previous = Some(here);
                if here <= target { selected = Some((here, header)); }
            }
            let (found, entry) = selected.ok_or(())?;
            if level == 0 {
                let offset = short(&entry[17..19]);
                let len = short(&entry[19..21]);
                if offset < end || len > 6144 { return Err(()); }
                return Ok((found, self.scratch.get(offset..offset + len).ok_or(())?));
            }
            ptr[..20].copy_from_slice(&entry[17..37]);
            ptr[20] = level - 1;
            upper = generation;
        }
    }

    fn exact(&mut self, ptr: Ptr, tree: u8, target: Key) -> Result<&[u8], ()> {
        let (found, bytes) = self.item(ptr, tree, target)?;
        if found != target { return Err(()); }
        Ok(bytes)
    }

    fn child(&mut self, tree: Ptr, parent: u64, name: &[u8], kind: u8) -> Result<u64, ()> {
        let hash = siphash::hash(&self.hash_key, name);
        let mut entries = self.exact(tree, 1, (parent, 0x10, hash))?;
        let mut found = None;
        while !entries.is_empty() {
            let len = usize::from(*entries.get(9).ok_or(())?);
            let candidate = entries.get(10..10 + len).ok_or(())?;
            if candidate == name {
                if entries[8] != kind || found.is_some() { return Err(()); }
                found = Some(wide(&entries[..8]));
            }
            entries = &entries[10 + len..];
        }
        found.ok_or(())
    }

    fn inode(&mut self, tree: Ptr, inode: u64, file_type: u32) -> Result<&[u8], ()> {
        let bytes = self.exact(tree, 1, (inode, 1, 0))?;
        if bytes.len() != 128 || word(&bytes[..4]) & 0o170022 != file_type
            || word(&bytes[8..12]) != 0 || word(&bytes[4..8]) == 0
        { return Err(()); }
        Ok(bytes)
    }

    pub fn file(&mut self, name: &[u8]) -> Result<File, ()> {
        let root = self.exact(self.space, 2, (0xffff_ffff_ffff_ff10, 0x90, 1))?;
        let tree = root.get(..24).ok_or(())?.try_into().unwrap();
        self.inode(tree, 2, 0o040000)?;
        let home = self.child(tree, 2, b"root", 2)?;
        self.inode(tree, home, 0o040000)?;
        let staged = self.child(tree, home, b".breenix-flash", 2)?;
        self.inode(tree, staged, 0o040000)?;
        let inode = self.child(tree, staged, name, 1)?;
        let data = self.inode(tree, inode, 0o100000)?;
        if word(&data[4..8]) != 1 { return Err(()); }
        Ok(File { tree, inode, size: usize::try_from(wide(&data[16..24])).map_err(|_| ())? })
    }

    /// Output must have space through the end of the last disk block.
    pub fn read_file(&mut self, file: File, output: &mut [u8]) -> Result<(), ()> {
        let capacity = file.size.div_ceil(BLOCK).checked_mul(BLOCK).ok_or(())?;
        let output = output.get_mut(..capacity).ok_or(())?;
        let mut offset = 0;
        while offset < output.len() {
            let (found, extent) = self.item(file.tree, 1, (file.inode, 0x20, offset as u64))?;
            if found.0 != file.inode || found.1 != 0x20 || found.2 % BLOCK as u64 != 0
                || extent.len() != 24 || word(&extent[20..24]) != 0
            { return Err(()); }
            let address = wide(&extent[..8]);
            let length = word(&extent[8..12]) as usize;
            let start = word(&extent[12..16]) as usize;
            let count = word(&extent[16..20]) as usize;
            let skip = (offset - found.2 as usize) / BLOCK;
            if length > 256 || address < 20 || address > self.total - 4
                || length as u64 > self.total - 4 - address || start > length || count > length - start
                || skip >= count
            { return Err(()); }
            let blocks = (count - skip).min((output.len() - offset) / BLOCK);
            let mut checksums = [0u32; 256];
            let reference = self.exact(self.space, 2, (address, 0x80, length as u64))?;
            if reference.len() != 16 + 4 * length || reference[4] != 3 { return Err(()); }
            let from = start + skip;
            for (i, checksum) in checksums[..blocks].iter_mut().enumerate() {
                let at = 16 + (from + i) * 4;
                *checksum = word(&reference[at..at + 4]);
            }
            let bytes = &mut output[offset..offset + blocks * BLOCK];
            (self.read)(address + from as u64, bytes)?;
            for (block, expected) in bytes.chunks_exact(BLOCK).zip(checksums) {
                if crc32c::checksum(block) != expected { return Err(()); }
            }
            offset += bytes.len();
        }
        Ok(())
    }
}
