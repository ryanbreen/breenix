//! Mutation fixtures use the writer's codecs; the checker remains independent.

use cowfs::crc32c::checksum;
use cowfs::format::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const SIZE: usize = 64 * 1024 * 1024;
const ROOT_OBJECT: u64 = 0xffff_ffff_ffff_ff10;
const FREE_OBJECT: u64 = 0xffff_ffff_ffff_ff20;
const PENDING_OBJECT: u64 = 0xffff_ffff_ffff_ff30;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct Corpus {
    dir: PathBuf,
    pub empty: PathBuf,
    pub populated: PathBuf,
}

impl Corpus {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "cowfs-violations-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let empty = dir.join("empty.img");
        let populated = dir.join("populated.img");
        mkfs(&empty, false);
        mkfs(&populated, true);
        Self {
            dir,
            empty,
            populated,
        }
    }

    pub fn violation(&self, name: &str) -> PathBuf {
        let mut image = Image::read(&self.populated);
        image.mutate(name);
        let path = self.dir.join(format!("{name}.img"));
        fs::write(&path, image.seal(name)).unwrap();
        path
    }
}

impl Drop for Corpus {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}

fn mkfs(path: &Path, populate: bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cowfs-mkfs"));
    command.args([
        "--uuid",
        "00112233445566778899aabbccddeeff",
        "--hash-key",
        "000102030405060708090a0b0c0d0e0f",
        "--time",
        "123456789",
        "--size",
        &SIZE.to_string(),
    ]);
    if populate {
        command
            .arg("--populate")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture"));
    }
    let output = command.arg(path).output().expect("run built cowfs-mkfs");
    assert!(
        output.status.success(),
        "mkfs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[derive(Clone)]
struct Item {
    key: Key,
    payload: Vec<u8>,
}

struct Leaf {
    header: NodeHeader,
    items: Vec<Item>,
}

impl Leaf {
    fn read(bytes: &[u8], ptr: BlockPtr) -> Self {
        let start = ptr.addr as usize * BLOCK_SIZE;
        let raw = &bytes[start..start + NODE_SIZE];
        let header = NodeHeader::decode(raw).unwrap();
        assert_eq!(header.level, 0, "small fixture must fit in one leaf");
        let items = (0..header.nitems as usize)
            .map(|i| {
                let h = LeafItemHeader::decode(&raw[48 + i * 21..]).unwrap();
                let start = h.data_off as usize;
                Item {
                    key: h.key,
                    payload: raw[start..start + h.data_len as usize].to_vec(),
                }
            })
            .collect();
        Self { header, items }
    }

    fn encode(&self) -> Vec<u8> {
        let mut raw = vec![0; NODE_SIZE];
        let mut end = NODE_SIZE;
        for (i, item) in self.items.iter().enumerate() {
            end -= item.payload.len();
            assert!(end >= 48 + self.items.len() * 21);
            raw[end..end + item.payload.len()].copy_from_slice(&item.payload);
            let h = LeafItemHeader {
                key: item.key,
                data_off: end as u16,
                data_len: item.payload.len() as u16,
            };
            raw[48 + i * 21..48 + (i + 1) * 21].copy_from_slice(&h.encode());
        }
        NodeHeader {
            nitems: self.items.len() as u16,
            ..self.header
        }
        .encode(&mut raw)
        .unwrap();
        raw
    }

    fn item_mut(&mut self, key: Key) -> &mut Item {
        self.items
            .iter_mut()
            .find(|item| item.key == key)
            .expect("fixture item")
    }

    fn inode(&self, ino: u64) -> Inode {
        Inode::decode(
            &self
                .items
                .iter()
                .find(|i| i.key == key(ino, INODE, 0))
                .unwrap()
                .payload,
        )
        .unwrap()
    }

    fn edit_inode(&mut self, ino: u64, edit: impl FnOnce(&mut Inode)) {
        let mut inode = self.inode(ino);
        edit(&mut inode);
        self.item_mut(key(ino, INODE, 0)).payload = inode.encode().to_vec();
    }
}

fn key(object: u64, item_type: u8, offset: u64) -> Key {
    Key {
        object,
        item_type,
        offset,
    }
}

struct Image {
    bytes: Vec<u8>,
    sb: Superblock,
    fs: Leaf,
    space: Leaf,
    root: Root,
    wrapper: Option<u64>,
}

impl Image {
    fn read(path: &Path) -> Self {
        let bytes = fs::read(path).unwrap();
        let sb = Superblock::decode(&bytes[17 * BLOCK_SIZE..18 * BLOCK_SIZE]).unwrap();
        let space = Leaf::read(&bytes, sb.space_root);
        let root = Root::decode(
            &space
                .items
                .iter()
                .find(|i| i.key.item_type == ROOT)
                .unwrap()
                .payload,
        )
        .unwrap();
        let fs = Leaf::read(&bytes, root.root);
        // The fixture's large file exercises both the maximum extent and a short tail.
        let large = Self::named(&fs, b"multi-extent.bin");
        assert_eq!(
            fs.items
                .iter()
                .filter(|i| i.key.object == large && i.key.item_type == EXTENT)
                .count(),
            2
        );
        Self {
            bytes,
            sb,
            fs,
            space,
            root,
            wrapper: None,
        }
    }

    fn named(fs: &Leaf, name: &[u8]) -> u64 {
        fs.items
            .iter()
            .filter(|i| i.key.item_type == DIR_INDEX)
            .find_map(|i| {
                let r = DirRecord::decode(&i.payload).unwrap();
                (r.name == name).then_some(r.ino)
            })
            .expect("fixture name")
    }

    fn free_key(&self) -> Key {
        self.space
            .items
            .iter()
            .find(|i| i.key.item_type == FREE)
            .unwrap()
            .key
    }

    /// Take a prefix of FREE, leaving accounting for the caller to supply.
    fn take_free(&mut self, blocks: u64) -> u64 {
        let k = self.free_key();
        let free = self.space.item_mut(k);
        let length = Free::decode(&free.payload).unwrap().length;
        assert!(length > blocks);
        free.key.offset += blocks;
        free.payload = Free {
            length: length - blocks,
        }
        .encode()
        .to_vec();
        k.offset
    }

    fn add_ref(&mut self, addr: u64, length: u64, kind: u8, refcount: u32) {
        let crcs = if kind == 3 {
            (addr..addr + length)
                .map(|b| {
                    let start = b as usize * BLOCK_SIZE;
                    checksum(&self.bytes[start..start + BLOCK_SIZE])
                })
                .collect()
        } else {
            Vec::new()
        };
        self.space.items.push(Item {
            key: key(addr, REF, length),
            payload: Ref {
                refcount,
                kind,
                alloc_generation: 1,
                crcs,
            }
            .encode(),
        });
    }

    fn mutate(&mut self, name: &str) {
        let file = Self::named(&self.fs, b"small.txt");
        match name {
            // Applied after the normal seal, so only the chosen field or pointer differs.
            "slot-reserved-byte"
            | "ring-copy-disagreement"
            | "no-matching-recovery-roots"
            | "root-pointer-crc"
            | "unused-node-byte" => {}
            "next-root-id" => self.sb.next_root_id = 1,
            "future-node-generation" => {
                self.sb.generation = 5;
                self.fs.header.generation = 6;
            }
            "child-level" => {
                // A level-2 root names the level-0 leaf directly, breaking only N3.
                let addr = self.take_free(4);
                self.add_ref(addr, 4, 1, 1);
                self.wrapper = Some(addr);
            }
            "leaf-key-order" => {} // Swap complete items after the ordinary sort.
            "inode-reserved-byte" => self.fs.item_mut(key(file, INODE, 0)).payload[112] = 1,
            "uncovered-block" => {
                self.take_free(1);
            }
            "adjacent-free-extents" => {
                let start = self.take_free(1);
                self.space.items.push(Item {
                    key: key(FREE_OBJECT, FREE, start),
                    payload: Free { length: 1 }.encode().to_vec(),
                });
            }
            "node-ref-kind" | "node-refcount" => {
                let item = self.space.item_mut(key(self.fs.header.addr, REF, 4));
                let mut r = Ref::decode(&item.payload).unwrap();
                if name == "node-ref-kind" {
                    r.kind = 2;
                } else {
                    r.refcount = 2;
                }
                item.payload = r.encode();
            }
            "unreferenced-zero-refcount" => {
                // An unreferenced REF with refcount 0 satisfies A4, isolating A5.
                let start = self.take_free(1);
                self.add_ref(start, 1, 3, 0);
            }
            "future-pending-generation" => {
                let start = self.take_free(1);
                self.space.items.push(Item {
                    key: key(PENDING_OBJECT, PENDING, start),
                    payload: Pending {
                        length: 1,
                        released_generation: self.sb.generation + 1,
                    }
                    .encode()
                    .to_vec(),
                });
            }
            "data-crc-count" => {
                let r = self
                    .space
                    .items
                    .iter_mut()
                    .find(|i| i.key.item_type == REF && Ref::decode(&i.payload).unwrap().kind == 3)
                    .unwrap();
                r.payload.truncate(r.payload.len() - 4);
            }
            "data-byte" => {
                let item = self
                    .fs
                    .items
                    .iter()
                    .find(|i| i.key.object == file && i.key.item_type == EXTENT)
                    .unwrap();
                let e = Extent::decode(&item.payload).unwrap();
                self.bytes[e.phys_start as usize * BLOCK_SIZE] ^= 1;
            }
            "root-parent" => self.fs.edit_inode(2, |i| i.parent = 16),
            "object-without-inode" => {
                let ino = self.root.next_inode;
                self.root.next_inode += 1;
                self.add_xattrs(ino, false);
            }
            "directory-name-hash" => {
                let entry = self
                    .fs
                    .items
                    .iter_mut()
                    .find(|i| i.key.item_type == DIR_ENTRY)
                    .unwrap();
                entry.key.offset ^= 1;
            }
            "directory-index-bound" => self.fs.edit_inode(2, |i| i.next_index = 2),
            "file-link-count" => self.fs.edit_inode(file, |i| i.nlink += 1),
            "directory-parent-cycle" => self.parent_cycle(),
            "nonzero-file-tail" => self.nonzero_tail(),
            "symlink-size" => {
                let ino = Self::named(&self.fs, b"link");
                self.fs.edit_inode(ino, |i| i.size += 1);
            }
            "duplicate-xattr-name" => self.add_xattrs(file, true),
            "next-inode" => self.root.next_inode -= 1,
            _ => panic!("unknown mutation {name}"),
        }
    }

    fn add_xattrs(&mut self, ino: u64, duplicate: bool) {
        let x = Xattr {
            name: b"user.fixture".to_vec(),
            value: b"value".to_vec(),
        };
        let mut payload = x.encode();
        if duplicate {
            payload.extend_from_slice(&x.encode());
        }
        self.fs.items.push(Item {
            key: key(
                ino,
                XATTR,
                cowfs::siphash::hash(&self.sb.dirhash_key, &x.name),
            ),
            payload,
        });
    }

    fn parent_cycle(&mut self) {
        let dir = Self::named(&self.fs, b"nested");
        // Move both records naming nested into nested itself and adjust the two
        // directories' counts and parent. F3-F5 and F8 still hold; the chain loops.
        let index = self.fs.inode(dir).next_index;
        for item in &mut self.fs.items {
            if item.key.object == 2 && matches!(item.key.item_type, DIR_ENTRY | DIR_INDEX) {
                let r = DirRecord::decode(&item.payload).unwrap();
                if r.ino == dir {
                    item.key.object = dir;
                    if item.key.item_type == DIR_INDEX {
                        item.key.offset = index;
                    }
                }
            }
        }
        self.fs.edit_inode(2, |i| {
            i.size -= 1;
            i.nlink -= 1;
        });
        self.fs.edit_inode(dir, |i| {
            i.size += 1;
            i.nlink += 1;
            i.next_index += 1;
            i.parent = dir;
        });
    }

    fn nonzero_tail(&mut self) {
        let ino = Self::named(&self.fs, b"multi-extent.bin");
        let size = self.fs.inode(ino).size;
        let item = self
            .fs
            .items
            .iter()
            .filter(|i| i.key.object == ino && i.key.item_type == EXTENT)
            .max_by_key(|i| i.key)
            .unwrap();
        let e = Extent::decode(&item.payload).unwrap();
        let block = e.phys_start + u64::from(e.extent_offset + e.num_blocks - 1);
        let start = block as usize * BLOCK_SIZE;
        self.bytes[start + size as usize % BLOCK_SIZE] = 1;
        // This is a valid checksummed byte outside EOF, isolating F7 from C1.
        let item = self
            .space
            .item_mut(key(e.phys_start, REF, u64::from(e.phys_len)));
        let mut r = Ref::decode(&item.payload).unwrap();
        r.crcs[(block - e.phys_start) as usize] = checksum(&self.bytes[start..start + BLOCK_SIZE]);
        item.payload = r.encode();
    }

    fn write_node(&mut self, header: NodeHeader, raw: &[u8]) -> BlockPtr {
        let start = header.addr as usize * BLOCK_SIZE;
        self.bytes[start..start + NODE_SIZE].copy_from_slice(raw);
        BlockPtr {
            addr: header.addr,
            generation: header.generation,
            csum: u32::from_le_bytes(raw[8..12].try_into().unwrap()),
            level: header.level,
        }
    }

    fn seal(mut self, name: &str) -> Vec<u8> {
        self.fs.items.sort_by_key(|i| i.key);
        if name == "leaf-key-order" {
            self.fs.items.swap(0, 1);
        }
        let mut raw = self.fs.encode();
        if name == "unused-node-byte" {
            raw[48 + self.fs.items.len() * 21] = 1;
            self.fs.header.encode(&mut raw).unwrap();
        }
        let mut fs_ptr = self.write_node(self.fs.header, &raw);
        if let Some(addr) = self.wrapper {
            let header = NodeHeader {
                addr,
                level: 2,
                nitems: 1,
                ..self.fs.header
            };
            let mut raw = vec![0; NODE_SIZE];
            let entry = InternalEntry {
                key: self.fs.items[0].key,
                child_addr: fs_ptr.addr,
                child_generation: fs_ptr.generation,
                child_csum: fs_ptr.csum,
            };
            raw[48..85].copy_from_slice(&entry.encode());
            header.encode(&mut raw).unwrap();
            fs_ptr = self.write_node(header, &raw);
        }
        if name == "root-pointer-crc" {
            // Recovery only checks ROOT pointers written in M. Keep this tree in
            // generation 1 and choose M=5 to exercise the ordinary N1 walk.
            self.sb.generation = 5;
            fs_ptr.csum ^= 1;
        }
        self.root.root = fs_ptr;
        self.space.item_mut(key(ROOT_OBJECT, ROOT, 1)).payload = self.root.encode();
        self.space.items.sort_by_key(|i| i.key);
        let raw = self.space.encode();
        self.sb.space_root = self.write_node(self.space.header, &raw);
        if name == "no-matching-recovery-roots" {
            self.sb.space_root.csum ^= 1;
        }
        self.sb.free_blocks = self
            .space
            .items
            .iter()
            .filter(|i| i.key.item_type == FREE)
            .map(|i| Free::decode(&i.payload).unwrap().length)
            .sum();
        for ring in 0..2 {
            self.sb.ring = ring;
            self.sb.slot = (self.sb.generation % 4) as u8;
            let mut raw = self.sb.encode();
            if name == "slot-reserved-byte" {
                raw[42] = 1;
            }
            if name == "ring-copy-disagreement" && ring == 1 {
                raw[200] = b'x';
            }
            let crc = checksum(&raw[12..]);
            raw[8..12].copy_from_slice(&crc.to_le_bytes());
            let block = if ring == 0 {
                16
            } else {
                self.sb.total_blocks - 4
            } + u64::from(self.sb.slot);
            let start = block as usize * BLOCK_SIZE;
            self.bytes[start..start + BLOCK_SIZE].copy_from_slice(&raw);
        }
        self.bytes
    }
}
