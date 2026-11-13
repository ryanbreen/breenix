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

fn superblock(ring: u8, generation: u64, space_root: [u8; 24]) -> Vec<u8> {
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
    put(&mut sb, 112, &(TOTAL - 4 - FIRST_FREE).to_le_bytes());
    // global_reserve = clamp(floor(16384 / 200), 4096, 262144)
    put(&mut sb, 120, &4096u64.to_le_bytes());
    put(&mut sb, 128, &2u64.to_le_bytes());
    put(&mut sb, 136, b"0123456789abcdef");
    put(&mut sb, 264, b"hand-built test");
    let crc = checksum(&sb[12..]);
    put(&mut sb, 8, &crc.to_le_bytes());
    sb
}

/// Generation 1 of a fresh filesystem: a SPACE leaf at block 20 and the default root's FS
/// leaf (inode 2 only) at block 24, and superblock 1 in slot 1 of rings A and B. The six
/// other slots are left as the sparse file's unwritten bytes.
fn clean_image(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("check-{name}.img"));
    let _ = fs::remove_file(&path);
    let file = File::create(&path).expect("create image");
    file.set_len(TOTAL * BLOCK as u64).expect("size image");

    let mut inode = vec![0u8; 128];
    put(&mut inode, 0, &0o040755u32.to_le_bytes());
    put(&mut inode, 4, &2u32.to_le_bytes());
    put(&mut inode, 32, &2u64.to_le_bytes());
    put(&mut inode, 48, &2u64.to_le_bytes());
    put(&mut inode, 56, &1u64.to_le_bytes());
    let fs_node = leaf(1, FS_NODE, 1, &[((2, 0x01, 0), inode)]);

    let node_ref = |kind: u8| {
        let mut r = vec![0u8; 16];
        put(&mut r, 0, &1u32.to_le_bytes());
        r[4] = kind;
        put(&mut r, 8, &1u64.to_le_bytes());
        r
    };
    let mut root = Vec::new();
    root.extend_from_slice(&block_ptr(FS_NODE, 1, node_crc(&fs_node), 0));
    root.extend_from_slice(&16u64.to_le_bytes());
    root.extend_from_slice(&0u64.to_le_bytes());
    root.extend_from_slice(&1u64.to_le_bytes());
    root.extend_from_slice(&[0u8; 12]);
    root.extend_from_slice(&0u32.to_le_bytes());
    root.push(7);
    root.extend_from_slice(b"default");
    let space_node = leaf(
        2,
        SPACE_NODE,
        1,
        &[
            ((SPACE_NODE, 0x80, 4), node_ref(2)),
            ((FS_NODE, 0x80, 4), node_ref(1)),
            ((ROOT_OBJECT, 0x90, 1), root),
            ((FREE_OBJECT, 0x81, FIRST_FREE), (TOTAL - 4 - FIRST_FREE).to_le_bytes().to_vec()),
        ],
    );

    let space_ptr = block_ptr(SPACE_NODE, 1, node_crc(&space_node), 0);
    file.write_all_at(&space_node, SPACE_NODE * BLOCK as u64).unwrap();
    file.write_all_at(&fs_node, FS_NODE * BLOCK as u64).unwrap();
    file.write_all_at(&superblock(0, 1, space_ptr), 17 * BLOCK as u64).unwrap();
    file.write_all_at(&superblock(1, 1, space_ptr), (TOTAL - 4 + 1) * BLOCK as u64).unwrap();
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
