//! Packed cowfs v1 disk structures, with structure-relative offsets.
use super::crc32c;
use alloc::vec::Vec;

pub const BLOCK_SIZE: usize = 4096;
pub const NODE_SIZE: usize = 16384;
pub const INODE: u8 = 1;
pub const DIR_ENTRY: u8 = 0x10;
pub const DIR_INDEX: u8 = 0x11;
pub const EXTENT: u8 = 0x20;
pub const INLINE: u8 = 0x21;
pub const XATTR: u8 = 0x30;
pub const ORPHAN: u8 = 0x40;
pub const REF: u8 = 0x80;
pub const FREE: u8 = 0x81;
pub const PENDING: u8 = 0x82;
pub const DEAD: u8 = 0x83;
pub const ROOT: u8 = 0x90;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated,
    Invalid,
    Checksum,
}
fn need(b: &[u8], n: usize) -> Result<(), Error> {
    if b.len() < n {
        Err(Error::Truncated)
    } else {
        Ok(())
    }
}
fn u16at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn put(b: &mut [u8], o: usize, v: &[u8]) {
    b[o..o + v.len()].copy_from_slice(v);
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Time {
    pub seconds: i64,
    pub nanos: u32,
}
fn get_time(b: &[u8], o: usize) -> Result<Time, Error> {
    let t = Time {
        seconds: u64at(b, o) as i64,
        nanos: u32at(b, o + 8),
    };
    if t.nanos >= 1_000_000_000 {
        return Err(Error::Invalid);
    }
    Ok(t)
}
fn put_time(b: &mut [u8], o: usize, t: Time) {
    put(b, o, &t.seconds.to_le_bytes());
    put(b, o + 8, &t.nanos.to_le_bytes());
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockPtr {
    pub addr: u64,
    pub generation: u64,
    pub csum: u32,
    pub level: u8,
}
impl BlockPtr {
    pub fn encode(&self) -> [u8; 24] {
        let mut b = [0; 24];
        put(&mut b, 0, &self.addr.to_le_bytes());
        put(&mut b, 8, &self.generation.to_le_bytes());
        put(&mut b, 16, &self.csum.to_le_bytes());
        b[20] = self.level;
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 24)?;
        if b[21..24] != [0; 3] {
            return Err(Error::Invalid);
        }
        Ok(Self {
            addr: u64at(b, 0),
            generation: u64at(b, 8),
            csum: u32at(b, 16),
            level: b[20],
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct Key {
    pub object: u64,
    pub item_type: u8,
    pub offset: u64,
}
impl Key {
    pub fn encode(&self) -> [u8; 17] {
        let mut b = [0; 17];
        put(&mut b, 0, &self.object.to_le_bytes());
        b[8] = self.item_type;
        put(&mut b, 9, &self.offset.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 17)?;
        Ok(Self {
            object: u64at(b, 0),
            item_type: b[8],
            offset: u64at(b, 9),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Extent {
    pub phys_start: u64,
    pub phys_len: u32,
    pub extent_offset: u32,
    pub num_blocks: u32,
    pub flags: u32,
}
impl Extent {
    pub fn encode(&self) -> [u8; 24] {
        let mut b = [0; 24];
        put(&mut b, 0, &self.phys_start.to_le_bytes());
        put(&mut b, 8, &self.phys_len.to_le_bytes());
        put(&mut b, 12, &self.extent_offset.to_le_bytes());
        put(&mut b, 16, &self.num_blocks.to_le_bytes());
        put(&mut b, 20, &self.flags.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 24)?;
        Ok(Self {
            phys_start: u64at(b, 0),
            phys_len: u32at(b, 8),
            extent_offset: u32at(b, 12),
            num_blocks: u32at(b, 16),
            flags: u32at(b, 20),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Free {
    pub length: u64,
}
impl Free {
    pub fn encode(&self) -> [u8; 8] {
        let mut b = [0; 8];
        put(&mut b, 0, &self.length.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 8)?;
        Ok(Self {
            length: u64at(b, 0),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Pending {
    pub length: u64,
    pub released_generation: u64,
}
impl Pending {
    pub fn encode(&self) -> [u8; 16] {
        let mut b = [0; 16];
        put(&mut b, 0, &self.length.to_le_bytes());
        put(&mut b, 8, &self.released_generation.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 16)?;
        Ok(Self {
            length: u64at(b, 0),
            released_generation: u64at(b, 8),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LeafItemHeader {
    pub key: Key,
    pub data_off: u16,
    pub data_len: u16,
}
impl LeafItemHeader {
    pub fn encode(&self) -> [u8; 21] {
        let mut b = [0; 21];
        put(&mut b, 0, &self.key.encode());
        put(&mut b, 17, &self.data_off.to_le_bytes());
        put(&mut b, 19, &self.data_len.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 21)?;
        Ok(Self {
            key: Key::decode(b)?,
            data_off: u16at(b, 17),
            data_len: u16at(b, 19),
        })
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InternalEntry {
    pub key: Key,
    pub child_addr: u64,
    pub child_generation: u64,
    pub child_csum: u32,
}
impl InternalEntry {
    pub fn encode(&self) -> [u8; 37] {
        let mut b = [0; 37];
        put(&mut b, 0, &self.key.encode());
        put(&mut b, 17, &self.child_addr.to_le_bytes());
        put(&mut b, 25, &self.child_generation.to_le_bytes());
        put(&mut b, 33, &self.child_csum.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 37)?;
        Ok(Self {
            key: Key::decode(b)?,
            child_addr: u64at(b, 17),
            child_generation: u64at(b, 25),
            child_csum: u32at(b, 33),
        })
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Inode {
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub nblocks: u64,
    pub parent: u64,
    pub rdev: u64,
    pub next_index: u64,
    pub created_generation: u64,
    pub atime: Time,
    pub mtime: Time,
    pub ctime: Time,
    pub btime: Time,
}
impl Inode {
    pub fn encode(&self) -> [u8; 128] {
        let mut b = [0; 128];
        put(&mut b, 0, &self.mode.to_le_bytes());
        put(&mut b, 4, &self.nlink.to_le_bytes());
        put(&mut b, 8, &self.uid.to_le_bytes());
        put(&mut b, 12, &self.gid.to_le_bytes());
        put(&mut b, 16, &self.size.to_le_bytes());
        put(&mut b, 24, &self.nblocks.to_le_bytes());
        put(&mut b, 32, &self.parent.to_le_bytes());
        put(&mut b, 40, &self.rdev.to_le_bytes());
        put(&mut b, 48, &self.next_index.to_le_bytes());
        put(&mut b, 56, &self.created_generation.to_le_bytes());
        put_time(&mut b, 64, self.atime);
        put_time(&mut b, 76, self.mtime);
        put_time(&mut b, 88, self.ctime);
        put_time(&mut b, 100, self.btime);
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 128)?;
        if b[112..128] != [0; 16] {
            return Err(Error::Invalid);
        }
        Ok(Self {
            mode: u32at(b, 0),
            nlink: u32at(b, 4),
            uid: u32at(b, 8),
            gid: u32at(b, 12),
            size: u64at(b, 16),
            nblocks: u64at(b, 24),
            parent: u64at(b, 32),
            rdev: u64at(b, 40),
            next_index: u64at(b, 48),
            created_generation: u64at(b, 56),
            atime: get_time(b, 64)?,
            mtime: get_time(b, 76)?,
            ctime: get_time(b, 88)?,
            btime: get_time(b, 100)?,
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Superblock {
    pub fs_uuid: [u8; 16],
    pub generation: u64,
    pub ring: u8,
    pub slot: u8,
    pub total_blocks: u64,
    pub compat: u64,
    pub ro_compat: u64,
    pub incompat: u64,
    pub space_root: BlockPtr,
    pub free_blocks: u64,
    pub global_reserve: u64,
    pub next_root_id: u64,
    pub dirhash_key: [u8; 16],
    pub commit_time: Time,
    pub mkfs_time: Time,
    pub last_scrub_generation: u64,
    pub last_scrub_errors: u64,
    pub label: [u8; 64],
    pub writer: [u8; 32],
}
impl Default for Superblock {
    fn default() -> Self {
        Self {
            fs_uuid: [0; 16],
            generation: 0,
            ring: 0,
            slot: 0,
            total_blocks: 0,
            compat: 0,
            ro_compat: 0,
            incompat: 1,
            space_root: BlockPtr::default(),
            free_blocks: 0,
            global_reserve: 0,
            next_root_id: 0,
            dirhash_key: [0; 16],
            commit_time: Time::default(),
            mkfs_time: Time::default(),
            last_scrub_generation: 0,
            last_scrub_errors: 0,
            label: [0; 64],
            writer: [0; 32],
        }
    }
}
impl Superblock {
    pub fn encode(&self) -> [u8; BLOCK_SIZE] {
        let mut b = [0; BLOCK_SIZE];
        put(&mut b, 0, b"CoWFS-SB");
        put(&mut b, 12, &1u32.to_le_bytes());
        put(&mut b, 16, &self.fs_uuid);
        put(&mut b, 32, &self.generation.to_le_bytes());
        b[40] = self.ring;
        b[41] = self.slot;
        put(&mut b, 44, &4096u32.to_le_bytes());
        put(&mut b, 48, &16384u32.to_le_bytes());
        for (o, v) in [
            (56, self.total_blocks),
            (64, self.compat),
            (72, self.ro_compat),
            (80, self.incompat),
            (112, self.free_blocks),
            (120, self.global_reserve),
            (128, self.next_root_id),
            (184, self.last_scrub_generation),
            (192, self.last_scrub_errors),
        ] {
            put(&mut b, o, &v.to_le_bytes());
        }
        put(&mut b, 88, &self.space_root.encode());
        put(&mut b, 136, &self.dirhash_key);
        put_time(&mut b, 152, self.commit_time);
        put_time(&mut b, 168, self.mkfs_time);
        put(&mut b, 200, &self.label);
        put(&mut b, 264, &self.writer);
        let c = crc32c::checksum(&b[12..]);
        put(&mut b, 8, &c.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, BLOCK_SIZE)?;
        if &b[0..8] != b"CoWFS-SB"
            || u32at(b, 12) != 1
            || u32at(b, 44) != 4096
            || u32at(b, 48) != 16384
            || b[42..44] != [0; 2]
            || b[52..56] != [0; 4]
            || b[164..168] != [0; 4]
            || b[180..184] != [0; 4]
            || b[296..BLOCK_SIZE].iter().any(|&x| x != 0)
        {
            return Err(Error::Invalid);
        }
        if crc32c::checksum(&b[12..BLOCK_SIZE]) != u32at(b, 8) {
            return Err(Error::Checksum);
        }
        let mut s = Self::default();
        s.fs_uuid.copy_from_slice(&b[16..32]);
        s.generation = u64at(b, 32);
        s.ring = b[40];
        s.slot = b[41];
        s.total_blocks = u64at(b, 56);
        s.compat = u64at(b, 64);
        s.ro_compat = u64at(b, 72);
        s.incompat = u64at(b, 80);
        s.space_root = BlockPtr::decode(&b[88..112])?;
        s.free_blocks = u64at(b, 112);
        s.global_reserve = u64at(b, 120);
        s.next_root_id = u64at(b, 128);
        s.dirhash_key.copy_from_slice(&b[136..152]);
        s.commit_time = get_time(b, 152)?;
        s.mkfs_time = get_time(b, 168)?;
        s.last_scrub_generation = u64at(b, 184);
        s.last_scrub_errors = u64at(b, 192);
        s.label.copy_from_slice(&b[200..264]);
        s.writer.copy_from_slice(&b[264..296]);
        Ok(s)
    }
}
pub fn valid_superblock_slot(b: &[u8], partition_blocks: u64, ring: u8, slot: u8) -> bool {
    match Superblock::decode(b) {
        Ok(s) => {
            s.generation > 0
                && ring < 2
                && slot < 4
                && s.ring == ring
                && s.slot == slot
                && u64::from(slot) == s.generation % 4
                && s.incompat == 1
                && (16384..=1u64 << 48).contains(&s.total_blocks)
                && s.total_blocks <= partition_blocks
        }
        Err(_) => false,
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NodeHeader {
    pub tree: u8,
    pub level: u8,
    pub nitems: u16,
    pub fs_uuid: [u8; 16],
    pub addr: u64,
    pub generation: u64,
}
impl NodeHeader {
    pub fn encode(&self, b: &mut [u8]) -> Result<(), Error> {
        need(b, NODE_SIZE)?;
        put(b, 0, b"CoWFS-ND");
        b[12] = self.tree;
        b[13] = self.level;
        put(b, 14, &self.nitems.to_le_bytes());
        put(b, 16, &self.fs_uuid);
        put(b, 32, &self.addr.to_le_bytes());
        put(b, 40, &self.generation.to_le_bytes());
        let c = crc32c::checksum(&b[12..NODE_SIZE]);
        put(b, 8, &c.to_le_bytes());
        Ok(())
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, NODE_SIZE)?;
        if &b[..8] != b"CoWFS-ND" {
            return Err(Error::Invalid);
        }
        if crc32c::checksum(&b[12..NODE_SIZE]) != u32at(b, 8) {
            return Err(Error::Checksum);
        }
        let mut fs_uuid = [0; 16];
        fs_uuid.copy_from_slice(&b[16..32]);
        Ok(Self {
            tree: b[12],
            level: b[13],
            nitems: u16at(b, 14),
            fs_uuid,
            addr: u64at(b, 32),
            generation: u64at(b, 40),
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirRecord {
    pub ino: u64,
    pub kind: u8,
    pub name: Vec<u8>,
}
impl DirRecord {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = alloc::vec![0;10+self.name.len()];
        put(&mut b, 0, &self.ino.to_le_bytes());
        b[8] = self.kind;
        b[9] = self.name.len() as u8;
        put(&mut b, 10, &self.name);
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 10)?;
        let n = b[9] as usize;
        need(b, 10 + n)?;
        if n == 0
            || !(1..=7).contains(&b[8])
            || b[10..10 + n].contains(&0)
            || b[10..10 + n].contains(&b'/')
            || &b[10..10 + n] == b"."
            || &b[10..10 + n] == b".."
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            ino: u64at(b, 0),
            kind: b[8],
            name: b[10..10 + n].to_vec(),
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Root {
    pub root: BlockPtr,
    pub next_inode: u64,
    pub source_root: u64,
    pub created_generation: u64,
    pub ctime: Time,
    pub flags: u32,
    pub name: Vec<u8>,
}
impl Root {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = alloc::vec![0;65+self.name.len()];
        put(&mut b, 0, &self.root.encode());
        put(&mut b, 24, &self.next_inode.to_le_bytes());
        put(&mut b, 32, &self.source_root.to_le_bytes());
        put(&mut b, 40, &self.created_generation.to_le_bytes());
        put_time(&mut b, 48, self.ctime);
        put(&mut b, 60, &self.flags.to_le_bytes());
        b[64] = self.name.len() as u8;
        put(&mut b, 65, &self.name);
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 65)?;
        let n = b[64] as usize;
        need(b, 65 + n)?;
        if n == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            root: BlockPtr::decode(b)?,
            next_inode: u64at(b, 24),
            source_root: u64at(b, 32),
            created_generation: u64at(b, 40),
            ctime: get_time(b, 48)?,
            flags: u32at(b, 60),
            name: b[65..65 + n].to_vec(),
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ref {
    pub refcount: u32,
    pub kind: u8,
    pub alloc_generation: u64,
    pub crcs: Vec<u32>,
}
impl Ref {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = alloc::vec![0;16+self.crcs.len()*4];
        put(&mut b, 0, &self.refcount.to_le_bytes());
        b[4] = self.kind;
        put(&mut b, 8, &self.alloc_generation.to_le_bytes());
        for (i, c) in self.crcs.iter().enumerate() {
            put(&mut b, 16 + i * 4, &c.to_le_bytes());
        }
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 16)?;
        if b[5..8] != [0; 3]
            || !(1..=3).contains(&b[4])
            || (b.len() - 16) % 4 != 0
            || (b[4] != 3 && b.len() != 16)
            || b.len() > 16 + 256 * 4
        {
            return Err(Error::Invalid);
        }
        let mut crcs = Vec::new();
        for i in (16..b.len()).step_by(4) {
            crcs.push(u32at(b, i));
        }
        Ok(Self {
            refcount: u32at(b, 0),
            kind: b[4],
            alloc_generation: u64at(b, 8),
            crcs,
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Xattr {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}
impl Xattr {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = alloc::vec![0;3+self.name.len()+self.value.len()];
        b[0] = self.name.len() as u8;
        put(&mut b, 1, &(self.value.len() as u16).to_le_bytes());
        put(&mut b, 3, &self.name);
        put(&mut b, 3 + self.name.len(), &self.value);
        b
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        need(b, 3)?;
        let n = b[0] as usize;
        let v = u16at(b, 1) as usize;
        need(b, 3 + n + v)?;
        if n == 0 || v > 4096 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            name: b[3..3 + n].to_vec(),
            value: b[3 + n..3 + n + v].to_vec(),
        })
    }
}
