//! cowfs-check's exit and output contract (docs/fs/cowfs-format.md section 5), on images
//! built here by hand from the format document's tables. Only the shared CRC-32C is used.

use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::process::Command;

use cowfs::crc32c::checksum;

const BLOCK: usize = 4096;
const NODE: usize = 16384;
const TOTAL: u64 = 16384;
const UUID: [u8; 16] = *b"hand-built-uuid!";
const SPACE_NODE: u64 = 20;
const FS_NODE: u64 = 24;
const FIRST_FREE: u64 = 28;
const ROOT_OBJECT: u64 = 0xFFFF_FFFF_FFFF_FF10;
const FREE_OBJECT: u64 = 0xFFFF_FFFF_FFFF_FF20;
const DIRHASH_KEY: &[u8; 16] = b"0123456789abcdef";

fn put(buf: &mut [u8], off: usize, bytes: &[u8]) {
    buf[off..off + bytes.len()].copy_from_slice(bytes);
}

fn block_ptr(addr: u64, generation: u64, csum: u32, level: u8) -> [u8; 24] {
    let mut p = [0u8; 24];
    put(&mut p, 0, &addr.to_le_bytes());
    put(&mut p, 8, &generation.to_le_bytes());
    put(&mut p, 16, &csum.to_le_bytes());
    p[20] = level;
    p
}

/// A leaf node: header, item headers in key order, payloads packed from the node's end.
fn leaf(tree: u8, addr: u64, generation: u64, items: &[((u64, u8, u64), Vec<u8>)]) -> Vec<u8> {
    let mut node = vec![0u8; NODE];
    put(&mut node, 0, b"CoWFS-ND");
    node[12] = tree;
    node[13] = 0;
    put(&mut node, 14, &(items.len() as u16).to_le_bytes());
    put(&mut node, 16, &UUID);
    put(&mut node, 32, &addr.to_le_bytes());
    put(&mut node, 40, &generation.to_le_bytes());
    let mut end = NODE;
    for (i, ((object, ty, offset), payload)) in items.iter().enumerate() {
        let h = 48 + 21 * i;
        end -= payload.len();
        put(&mut node, h, &object.to_le_bytes());
        node[h + 8] = *ty;
        put(&mut node, h + 9, &offset.to_le_bytes());
        put(&mut node, h + 17, &(end as u16).to_le_bytes());
        put(&mut node, h + 19, &(payload.len() as u16).to_le_bytes());
        put(&mut node, end, payload);
    }
    let crc = checksum(&node[12..]);
    put(&mut node, 8, &crc.to_le_bytes());
    node
}

fn node_crc(node: &[u8]) -> u32 {
    u32::from_le_bytes(node[8..12].try_into().unwrap())
}

fn superblock(ring: u8, generation: u64, space_root: [u8; 24], free_blocks: u64) -> Vec<u8> {
    let mut sb = vec![0u8; BLOCK];
    put(&mut sb, 0, b"CoWFS-SB");
    put(&mut sb, 12, &1u32.to_le_bytes());
    put(&mut sb, 16, &UUID);
    put(&mut sb, 32, &generation.to_le_bytes());
    sb[40] = ring;
    sb[41] = (generation % 4) as u8;
    put(&mut sb, 44, &4096u32.to_le_bytes());
    put(&mut sb, 48, &16384u32.to_le_bytes());
    put(&mut sb, 56, &TOTAL.to_le_bytes());
    put(&mut sb, 80, &1u64.to_le_bytes());
    put(&mut sb, 88, &space_root);
    put(&mut sb, 112, &free_blocks.to_le_bytes());
    // global_reserve = clamp(floor(16384 / 200), 4096, 262144)
    put(&mut sb, 120, &4096u64.to_le_bytes());
    put(&mut sb, 128, &2u64.to_le_bytes());
    put(&mut sb, 136, DIRHASH_KEY);
    put(&mut sb, 264, b"hand-built test");
    let crc = checksum(&sb[12..]);
    put(&mut sb, 8, &crc.to_le_bytes());
    sb
}

type Item = ((u64, u8, u64), Vec<u8>);

/// A 128-byte directory inode (format section 2.3) with the given link count, size (its
/// entry count), parent and next_index.
fn dir_inode(nlink: u32, size: u64, parent: u64, next_index: u64) -> Vec<u8> {
    let mut inode = vec![0u8; 128];
    put(&mut inode, 0, &0o040755u32.to_le_bytes());
    put(&mut inode, 4, &nlink.to_le_bytes());
    put(&mut inode, 16, &size.to_le_bytes());
    put(&mut inode, 32, &parent.to_le_bytes());
    put(&mut inode, 48, &next_index.to_le_bytes());
    put(&mut inode, 56, &1u64.to_le_bytes());
    inode
}

/// One directory record: child ino, kind, name_len, name.
fn dir_record(child: u64, kind: u8, name: &[u8]) -> Vec<u8> {
    let mut r = child.to_le_bytes().to_vec();
    r.push(kind);
    r.push(name.len() as u8);
    r.extend_from_slice(name);
    r
}

/// What a hand-built generation-1 image holds beyond its fixed layout.
struct Spec {
    /// The default root's FS leaf items, in key order.
    fs: Vec<Item>,
    /// FREE extents as (start, length); `free_blocks` is their sum.
    free: Vec<(u64, u64)>,
    next_inode: u64,
}

impl Spec {
    /// A fresh filesystem: inode 2 alone, and blocks 28 up to ring B in one FREE extent.
    fn fresh() -> Spec {
        Spec {
            fs: vec![((2, 0x01, 0), dir_inode(2, 0, 2, 2))],
            free: vec![(FIRST_FREE, TOTAL - 4 - FIRST_FREE)],
            next_inode: 16,
        }
    }
}

/// Generation 1 of a fresh filesystem: a SPACE leaf at block 20 and the default root's FS
/// leaf (inode 2 only) at block 24, and superblock 1 in slot 1 of rings A and B. The six
/// other slots are left as the sparse file's unwritten bytes.
fn clean_image(name: &str) -> PathBuf {
    image(name, Spec::fresh())
}

/// The fresh layout with `spec`'s FS items, FREE extents and next_inode.
fn image(name: &str, spec: Spec) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("check-{name}.img"));
    let _ = fs::remove_file(&path);
    let file = File::create(&path).expect("create image");
    file.set_len(TOTAL * BLOCK as u64).expect("size image");

    let fs_node = leaf(1, FS_NODE, 1, &spec.fs);

    let node_ref = |kind: u8| {
        let mut r = vec![0u8; 16];
        put(&mut r, 0, &1u32.to_le_bytes());
        r[4] = kind;
        put(&mut r, 8, &1u64.to_le_bytes());
        r
    };
    let mut root = Vec::new();
    root.extend_from_slice(&block_ptr(FS_NODE, 1, node_crc(&fs_node), 0));
    root.extend_from_slice(&spec.next_inode.to_le_bytes());
    root.extend_from_slice(&0u64.to_le_bytes());
    root.extend_from_slice(&1u64.to_le_bytes());
    root.extend_from_slice(&[0u8; 12]);
    root.extend_from_slice(&0u32.to_le_bytes());
    root.push(7);
    root.extend_from_slice(b"default");
    let mut space: Vec<Item> = vec![
        ((SPACE_NODE, 0x80, 4), node_ref(2)),
        ((FS_NODE, 0x80, 4), node_ref(1)),
        ((ROOT_OBJECT, 0x90, 1), root),
    ];
    let mut free = spec.free;
    free.sort_unstable();
    for &(start, length) in &free {
        space.push(((FREE_OBJECT, 0x81, start), length.to_le_bytes().to_vec()));
    }
    let free_blocks = free.iter().map(|&(_, length)| length).sum();
    let space_node = leaf(2, SPACE_NODE, 1, &space);

    let space_ptr = block_ptr(SPACE_NODE, 1, node_crc(&space_node), 0);
    file.write_all_at(&space_node, SPACE_NODE * BLOCK as u64).unwrap();
    file.write_all_at(&fs_node, FS_NODE * BLOCK as u64).unwrap();
    file.write_all_at(&superblock(0, 1, space_ptr, free_blocks), 17 * BLOCK as u64).unwrap();
    file.write_all_at(&superblock(1, 1, space_ptr, free_blocks), (TOTAL - 4 + 1) * BLOCK as u64).unwrap();
    path
}

fn flip(path: &PathBuf, byte: u64) {
    let file = fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
    let mut b = [0u8; 1];
    file.read_exact_at(&mut b, byte).unwrap();
    b[0] ^= 0x01;
    file.write_all_at(&b, byte).unwrap();
}

fn check(path: &PathBuf) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cowfs-check")).arg(path).output().expect("run cowfs-check");
    (out.status.code().expect("exit status"), String::from_utf8(out.stdout).expect("utf-8 output"))
}

#[test]
fn no_valid_slot_exits_8() {
    let zero = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("check-zero.img");
    File::create(&zero).unwrap().set_len(TOTAL * BLOCK as u64).unwrap();
    assert_eq!(check(&zero).0, 8, "an image with every slot zero");

    let torn = clean_image("no-valid-slot");
    flip(&torn, 17 * BLOCK as u64 + 8);
    flip(&torn, (TOTAL - 3) * BLOCK as u64 + 8);
    assert_eq!(check(&torn).0, 8, "both copies of the only generation fail their CRC");

    let missing = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("check-does-not-exist.img");
    assert_eq!(check(&missing).0, 8, "an unreadable file");
}

#[test]
fn bad_superblock_crc_names_s1() {
    let image = clean_image("bad-sb-crc");
    let (code, out) = check(&image);
    assert_eq!((code, out.as_str()), (0, ""), "the hand-built image is clean before the flip");

    flip(&image, 17 * BLOCK as u64 + 8);
    let (code, out) = check(&image);
    assert_eq!(code, 4, "output:\n{out}");
    let errors: Vec<&str> = out.lines().filter(|l| l.starts_with("error ")).collect();
    assert!(!errors.is_empty(), "output:\n{out}");
    assert!(errors.iter().all(|l| l.starts_with("error S1 ")), "only S1 is violated:\n{out}");
    assert!(errors.iter().any(|l| l.contains("block 17")), "the finding names the slot's block:\n{out}");
}

/// The invariant ids of the image's error lines.
fn error_ids(out: &str) -> Vec<&str> {
    out.lines().filter_map(|l| l.strip_prefix("error ")).filter_map(|l| l.split(' ').next()).collect()
}

#[test]
fn overlapping_free_names_a2() {
    // Blocks 100-107 lie in the main FREE extent and in a second one. free_blocks is the
    // sum of both, so S4 holds and only the space invariants are broken: A2 (FREE extents
    // that should be one) and A1 (blocks in more than one extent).
    let mut spec = Spec::fresh();
    spec.free.push((100, 8));
    let (code, out) = check(&image("overlapping-free", spec));
    assert_eq!(code, 4, "output:\n{out}");
    let ids = error_ids(&out);
    assert!(ids.contains(&"A2"), "A2 is named:\n{out}");
    assert!(ids.iter().all(|id| *id == "A1" || *id == "A2"), "only space coverage is violated:\n{out}");
    assert!(out.lines().any(|l| l.starts_with("error A2 ") && l.contains("100")), "A2 names the extent:\n{out}");
}

#[test]
fn dangling_dir_entry_names_f3() {
    // The root directory names "ghost", inode 16, through a DIR_ENTRY at SipHash(name) and
    // its DIR_INDEX, but the tree holds no inode 16.
    let name = b"ghost";
    let hash = cowfs::siphash::hash(DIRHASH_KEY, name);
    let record = dir_record(16, 1, name);
    let mut spec = Spec::fresh();
    spec.fs = vec![
        ((2, 0x01, 0), dir_inode(2, 1, 2, 3)),
        ((2, 0x10, hash), record.clone()),
        ((2, 0x11, 2), record),
    ];
    spec.next_inode = 17;
    let (code, out) = check(&image("dangling-dir-entry", spec));
    assert_eq!(code, 4, "output:\n{out}");
    let ids = error_ids(&out);
    assert!(!ids.is_empty() && ids.iter().all(|id| *id == "F3"), "only F3 is violated:\n{out}");
    assert!(out.lines().any(|l| l.contains("ghost") && l.contains("16")), "F3 names the entry and inode:\n{out}");
}
