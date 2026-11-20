//! Boot type detection uses the production probes and real filesystem images.
#[path = "../../../kernel/src/fs/breenfs/probe.rs"]
mod breenfs_probe;
use crate::{block::BlockDevice, errno::Errno};

fn kind(device: &dyn BlockDevice) -> Result<Option<&'static str>, Errno> {
    for (kind, probe) in [
        ("ext2", crate::ext2_format::FORMAT.probe),
        (
            "breenfs",
            breenfs_probe::probe as fn(&dyn BlockDevice) -> Result<bool, Errno>,
        ),
    ] {
        if probe(device)? {
            return Ok(Some(kind));
        }
    }
    Ok(None)
}

#[test]
fn ext2_first_and_breenfs_in_either_ring() {
    use crate::{
        breenfs::{
            commit::{Committer, DeviceStore},
            format::{Superblock, BLOCK_SIZE},
            ops::Files,
        },
        read_conformance::Disk,
    };
    use std::sync::Mutex;
    let ext2 = crate::host_mounts::Fixture::new();
    let device = ext2.device_image();
    assert_eq!(kind(device.as_ref()), Ok(Some("ext2")));
    assert_eq!(
        breenfs_probe::probe(device.as_ref()),
        Ok(false),
        "breenfs must never claim ext2"
    );
    let disk = Disk {
        bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
        writable: true,
    };
    let sb = Superblock {
        fs_uuid: [0x61; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        global_reserve: 4096,
        next_root_id: 2,
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    files.fsync(&disk, &mut Committer::format(sb)).unwrap();
    assert_eq!(kind(&disk), Ok(Some("breenfs")));
    disk.bytes.lock().unwrap()[16 * BLOCK_SIZE..20 * BLOCK_SIZE].fill(0);
    assert_eq!(
        kind(&disk),
        Ok(Some("breenfs")),
        "ring B alone must be detected"
    );
    assert_eq!(kind(crate::host_mounts::zeroed_device().as_ref()), Ok(None));
}

// Same boot selection, record parser, partition translations and BreenFS
// recovery as x86 metal. The sparse host disk supplies the sectors and GPT hit.
#[path = "../../../kernel/src/block/partition.rs"]
mod partition;
use crate::block::{BlockError, HardwareDisk};

mod restage {
    use crate::block::{install, BlockDevice, BlockError, HardwareDisk};
    use crate::breenfs::{commit::{Committer, DeviceStore}, format::{Superblock, BLOCK_SIZE},
        ops::Files, recover::recover};
    use std::{collections::BTreeMap, sync::{atomic::AtomicU8, Arc, Mutex}};

    use super::partition;

    const FIRST: u64 = 4_196_352;
    const SECTORS: u64 = 8_388_608;
    const PER_BLOCK: u64 = (BLOCK_SIZE / install::SECTOR) as u64;
    const IMAGE_BLOCKS: u64 = 73_728;

    #[derive(Clone, Default)]
    struct Disk(Arc<Mutex<BTreeMap<u64, [u8; install::SECTOR]>>>);

    impl BlockDevice for Disk {
        fn block_size(&self) -> usize { install::SECTOR }
        fn num_blocks(&self) -> u64 { FIRST + SECTORS }
        fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            if let Some(bytes) = self.0.lock().unwrap().get(&lba) {
                buf.copy_from_slice(bytes);
            } else {
                buf.fill(0);
            }
            Ok(())
        }
        fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
            assert!((FIRST..FIRST + SECTORS).contains(&lba));
            self.0.lock().unwrap().insert(lba, buf.try_into().unwrap());
            Ok(())
        }
        fn flush(&self) -> Result<(), BlockError> { Ok(()) }
    }

    mod gpt {
        pub const TEST_ROOT_TYPE: u8 = 1;
        pub const SYSTEM_TYPE: u8 = 2;
    }
    struct Entry { type_guid: u8, first: u64, last: u64 }
    const ROLE_TEST_ROOT: u8 = 1;
    const ROLE_SYSTEM: u8 = 2;
    static ROOT_ROLE: AtomicU8 = AtomicU8::new(0);
    std::thread_local! { static MEDIA: Disk = Disk::default(); }

    fn named_partition(_: &str, _: &[u8]) -> Result<(partition::Partition, Entry, HardwareDisk), &'static str> {
        let device = MEDIA.with(|disk| partition::Partition::new(Box::new(disk.clone()), FIRST, SECTORS))?;
        Ok((device, Entry { type_guid: gpt::TEST_ROOT_TYPE, first: FIRST, last: FIRST + SECTORS - 1 },
            HardwareDisk::Ahci(0)))
    }

    fn fence_add(_: HardwareDisk, first: u64, last: u64) -> Result<(), &'static str> {
        assert_eq!((first, last), (FIRST, FIRST + SECTORS - 1));
        Ok(())
    }

    mod intent {
        pub fn root_partition() -> Option<&'static str> { Some("test-root") }
        pub fn refused() -> Option<&'static str> { None }
    }

    include!(concat!(env!("OUT_DIR"), "/named_root.rs"));

    fn format_image(device: &dyn BlockDevice, uuid: [u8; 16], total_blocks: u64) {
        let sb = Superblock {
            fs_uuid: uuid,
            generation: 1,
            slot: 1,
            total_blocks,
            global_reserve: 4096,
            next_root_id: 2,
            ..Superblock::default()
        };
        let mut files = Files::format(DeviceStore(device), &sb).unwrap();
        files.fsync(device, &mut Committer::format(sb)).unwrap();
    }

    #[test]
    fn restaged_root_recovers_at_the_recorded_image_length() {
        let disk = MEDIA.with(Clone::clone);
        let partition = partition::Partition::new(Box::new(disk.clone()), FIRST, SECTORS).unwrap();
        let previous = partition::Partition::new(Box::new(disk.clone()), FIRST, 16_384 * PER_BLOCK).unwrap();
        format_image(&previous, [0x51; 16], 16_384);
        // Old writable recovery repairs B at the GPT tail rather than the image tail.
        let first = recover(&partition, true).unwrap();
        let stale_lba = FIRST + (SECTORS / PER_BLOCK - 4 + u64::from(first.slot)) * PER_BLOCK;
        let stale = disk.0.lock().unwrap()[&stale_lba];
        assert_eq!(&stale[16..32], &first.fs_uuid);

        // The stub rewrites only the prefix, then publishes LBA 0. Construct
        // that prefix here while preserving the stale physical tail.
        disk.0.lock().unwrap().retain(|&lba, _| lba >= FIRST + IMAGE_BLOCKS * PER_BLOCK);
        let image_sectors = IMAGE_BLOCKS * PER_BLOCK;
        let image = partition::Partition::new(Box::new(disk.clone()), FIRST, image_sectors).unwrap();
        let uuid = [0x52; 16];
        format_image(&image, uuid, IMAGE_BLOCKS);
        let mut hasher = install::sha256::Sha256::new();
        let sectors = disk.0.lock().unwrap();
        let zero = [0; install::SECTOR];
        for lba in FIRST + 1..FIRST + image_sectors {
            hasher.update(sectors.get(&lba).unwrap_or(&zero));
        }
        drop(sectors);
        let record = install::Record::Root {
            boot: "host-restage".into(),
            bytes: image_sectors * install::SECTOR as u64,
            sha256: hasher.finalize(),
        };
        image.write_block(0, &record.sector()).unwrap();

        let root = named_root().expect("the staged test root was selected");
        assert_eq!(recover(root.as_ref(), true).map(|sb| sb.fs_uuid), Ok(uuid));
        assert_eq!(root.num_blocks(), image_sectors);
        assert_eq!(root.write_block(image_sectors, &zero), Err(BlockError::OutOfBounds));
        assert_eq!(disk.0.lock().unwrap()[&stale_lba], stale);
    }
}
