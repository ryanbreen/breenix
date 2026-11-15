use super::{Event, RecordingDevice, SECTOR_SIZE};
use crate::{block::BlockDevice, format::*};
use alloc::{vec, vec::Vec};
use std::{fs, process::Command, sync::OnceLock};

// Run the real formatter once; the scripted commits below do not use a writer.
fn image() -> Vec<u8> {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let path = std::env::temp_dir()
                .join(std::format!("cowfs-recording-{}.img", std::process::id()));
            let source = path.with_extension("source");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("file"), [0x73; BLOCK_SIZE + 1]).unwrap();
            let output = Command::new(env!("CARGO_BIN_FILE_COWFS_cowfs-mkfs"))
                .args([
                    "--size",
                    "67108864",
                    "--time",
                    "0",
                    "--uuid",
                    "00112233445566778899aabbccddeeff",
                    "--hash-key",
                    "000102030405060708090a0b0c0d0e0f",
                ])
                .arg("--populate")
                .arg(&source)
                .arg(&path)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let bytes = fs::read(&path).unwrap();
            fs::remove_file(path).unwrap();
            fs::remove_dir_all(source).unwrap();
            bytes
        })
        .clone()
}

fn superblock(image: &[u8]) -> Superblock {
    Superblock::decode(&image[17 * BLOCK_SIZE..18 * BLOCK_SIZE]).unwrap()
}

fn write(device: &RecordingDevice, block: u64, bytes: &[u8]) {
    assert_eq!(bytes.len() % SECTOR_SIZE, 0);
    for (i, sector) in bytes.chunks_exact(SECTOR_SIZE).enumerate() {
        device.write_block(block * 8 + i as u64, sector).unwrap();
    }
}

fn publish(device: &RecordingDevice, sb: &Superblock) {
    for ring in 0..2 {
        let copy = Superblock { ring, ..sb.clone() };
        let block = if ring == 0 { 16 } else { sb.total_blocks - 4 } + u64::from(sb.slot);
        write(device, block, &copy.encode());
    }
}

// Clone the small mkfs space leaf at a fresh address. Its ROOT still names the
// old FS tree, exercising shared nodes across generations without a txn engine.
fn next(image: &[u8], generation: u64, addr: u64) -> (Superblock, Vec<u8>) {
    let mut sb = superblock(image);
    let lo = sb.space_root.addr as usize * BLOCK_SIZE;
    let mut node = image[lo..lo + NODE_SIZE].to_vec();
    let header = NodeHeader::decode(&node).unwrap();
    assert_eq!(header.level, 0);
    NodeHeader {
        addr,
        generation,
        ..header
    }
    .encode(&mut node)
    .unwrap();
    sb.generation = generation;
    sb.slot = (generation % 4) as u8;
    sb.space_root = BlockPtr {
        addr,
        generation,
        level: 0,
        csum: u32::from_le_bytes(node[8..12].try_into().unwrap()),
    };
    (sb, node)
}

fn rejects(rule: &str, f: impl FnOnce()) {
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_err();
    let message = panic
        .downcast_ref::<std::string::String>()
        .map(|s| s.as_str())
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains(rule), "{message}");
}

#[test]
fn w1_rejects_overwrite_of_live_block() {
    let image = image();
    let sb = superblock(&image);
    let device = RecordingDevice::new(image.clone()).unwrap();
    let (new_sb, node) = next(&image, 2, 100);
    write(&device, 100, &node);
    device.flush().unwrap();
    publish(&device, &new_sb);
    device.flush().unwrap();
    assert_eq!(device.durable_generations(), vec![1, 2]);
    // Only generation 1 names this old space node. Even its last sector is live.
    rejects("W1", || {
        device
            .write_block(sb.space_root.addr * 8 + 31, &[0xa5; SECTOR_SIZE])
            .unwrap();
    });

    // ROOT and EXTENT reachability must protect more than the space root itself.
    let space_lo = sb.space_root.addr as usize * BLOCK_SIZE;
    let space = &image[space_lo..space_lo + NODE_SIZE];
    let header = NodeHeader::decode(space).unwrap();
    let root_item = (0..usize::from(header.nitems))
        .find_map(|i| {
            let h = LeafItemHeader::decode(&space[48 + i * 21..69 + i * 21]).unwrap();
            (h.key.item_type == ROOT).then(|| {
                Root::decode(&space[h.data_off as usize..h.data_off as usize + h.data_len as usize])
                    .unwrap()
            })
        })
        .unwrap();
    let device = RecordingDevice::new(image.clone()).unwrap();
    rejects("W1", || {
        write(&device, root_item.root.addr + 3, &[0; BLOCK_SIZE])
    });
    let fs_lo = root_item.root.addr as usize * BLOCK_SIZE;
    let leaf = &image[fs_lo..fs_lo + NODE_SIZE];
    let header = NodeHeader::decode(leaf).unwrap();
    let extent = (0..usize::from(header.nitems))
        .find_map(|i| {
            let h = LeafItemHeader::decode(&leaf[48 + i * 21..69 + i * 21]).unwrap();
            (h.key.item_type == EXTENT).then(|| {
                Extent::decode(
                    &leaf[h.data_off as usize..h.data_off as usize + h.data_len as usize],
                )
                .unwrap()
            })
        })
        .unwrap();
    let device = RecordingDevice::new(image).unwrap();
    rejects("W1", || {
        write(
            &device,
            extent.phys_start + u64::from(extent.phys_len) - 1,
            &[0; BLOCK_SIZE],
        )
    });
}

#[test]
fn w2_rejects_superblock_before_flush() {
    let image = image();
    let device = RecordingDevice::new(image.clone()).unwrap();
    let (sb, node) = next(&image, 2, 100);
    write(&device, 100, &node);
    rejects("W2", || publish(&device, &sb));
}

#[test]
fn w3_durable_only_after_flush() {
    let image = image();
    let device = RecordingDevice::new(image.clone()).unwrap();
    let (sb, node) = next(&image, 2, 100);
    write(&device, 100, &node);
    device.flush().unwrap();
    let before = device.durable_image();
    publish(&device, &sb);
    assert_eq!(device.durable_generations(), vec![1]);
    assert_eq!(device.durable_image(), before);
    device.flush().unwrap();
    assert_eq!(device.durable_generations(), vec![1, 2]);
    let durable = device.durable_image();
    assert_eq!(&durable[18 * BLOCK_SIZE..19 * BLOCK_SIZE], &sb.encode());
    let mut sector = [0; SECTOR_SIZE];
    device.read_block(18 * 8, &mut sector).unwrap();
    assert_eq!(&sector, &sb.encode()[..SECTOR_SIZE]);
}

#[test]
fn legal_sequence_is_accepted() {
    let image = image();
    let device = RecordingDevice::new(image.clone()).unwrap();
    assert_eq!(device.block_size(), 512);
    assert_eq!(device.num_blocks(), image.len() as u64 / 512);
    for (generation, addr) in [(2, 100), (3, 104)] {
        let (sb, node) = next(&image, generation, addr);
        write(&device, addr, &node);
        device.flush().unwrap();
        publish(&device, &sb);
        device.flush().unwrap();
    }
    assert_eq!(device.durable_generations(), vec![2, 3]);
    // Generation 1 has aged out; its private space node can now be reused.
    let old = superblock(&image).space_root.addr;
    write(&device, old, &[0x5a; BLOCK_SIZE]);
    device.flush().unwrap();
    let events = device.events();
    assert_eq!(
        events.iter().filter(|e| matches!(e, Event::Flush)).count(),
        5
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Write { .. }))
            .count(),
        104
    );
    assert_eq!(events.last(), Some(&Event::Flush));
    assert!(
        matches!(&events[0], Event::Write { sector: 800, bytes } if bytes[..8] == *b"CoWFS-ND")
    );
}
