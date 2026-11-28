use crate::{
    attrs::InodeAttrs,
    block::{cache::BufferCache, BlockDevice, BlockError},
    breenfs::{
        commit::{slot_block, Error},
        format::{Superblock, BLOCK_SIZE},
        recover,
        vfs::{measurement::publication_waiter, BreenFs},
    },
    errno::Errno,
    read_conformance::NoWait,
    syscall_errno::EIO,
    trait_adapter::Device,
    vfs::{FileSystem, NewNode},
};
use breenfs_host_tests::recording::{Event, RecordingDevice, SECTOR_SIZE};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotOperation {
    Write { start: u64, sectors: usize },
    Flush,
}

struct SlotDisk {
    recording: Device,
    operations: Mutex<Vec<SlotOperation>>,
    fail: Option<SlotOperation>,
    write_limit: usize,
}

impl SlotDisk {
    fn record(&self, operation: SlotOperation) -> Result<(), BlockError> {
        let mut operations = self.operations.lock().unwrap();
        let is_fault = self.fail == Some(operation)
            && (operation != SlotOperation::Flush || operations.contains(&SlotOperation::Flush));
        operations.push(operation);
        if is_fault {
            Err(BlockError::IoError)
        } else {
            Ok(())
        }
    }
}

impl BlockDevice for SlotDisk {
    fn block_size(&self) -> usize {
        self.recording.block_size()
    }
    fn num_blocks(&self) -> u64 {
        self.recording.num_blocks()
    }
    fn max_single_write_bytes(&self) -> usize {
        self.write_limit
    }
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.recording.read_block(block, bytes)
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.write_blocks(block, 1, bytes)
    }
    fn write_blocks(&self, start: u64, count: usize, bytes: &[u8]) -> Result<(), BlockError> {
        let sectors_per_slot = (BLOCK_SIZE / self.block_size()) as u64;
        if (16 * sectors_per_slot..20 * sectors_per_slot).contains(&start)
            || start >= self.num_blocks() - 4 * sectors_per_slot
        {
            self.record(SlotOperation::Write {
                start,
                sectors: count,
            })?;
        }
        self.recording.write_blocks(start, count, bytes)
    }
    fn flush(&self) -> Result<(), BlockError> {
        self.record(SlotOperation::Flush)?;
        self.recording.flush()
    }
}

#[test]
fn fsync_publishes_whole_slots_between_flushes() {
    use SlotOperation::{Flush, Write};

    let image = crate::model::image();
    let previous = Superblock::decode(&image[17 * BLOCK_SIZE..18 * BLOCK_SIZE]).unwrap();
    let generation = previous.generation + 1;
    let slot = (previous.slot + 1) % 4;
    let sectors = BLOCK_SIZE / SECTOR_SIZE;
    let a = slot_block(previous.total_blocks, 0, slot) * sectors as u64;
    let b = slot_block(previous.total_blocks, 1, slot) * sectors as u64;
    let ring_a = Write { start: a, sectors };
    let ring_b = Write { start: b, sectors };
    for (write_limit, fail) in [
        (BLOCK_SIZE, None),
        (BLOCK_SIZE, Some(ring_b)),
        (BLOCK_SIZE, Some(Flush)),
        (0, None),
    ] {
        let disk = Arc::new(SlotDisk {
            recording: Device(Arc::new(RecordingDevice::new(image.clone()).unwrap())),
            operations: Mutex::new(Vec::new()),
            fail,
            write_limit,
        });
        let cache = Arc::new(BufferCache::new(NoWait));
        let fs = BreenFs::mount_writable(disk.clone(), cache.clone(), 77, 1).unwrap();
        let ino = fs
            .create(
                2,
                b"whole-slot",
                NewNode::Regular {
                    attrs: InodeAttrs::new(0o644, 0, 0),
                },
            )
            .unwrap();
        let file = fs.open(ino).unwrap();
        for start in [a, b] {
            for sector in start..start + sectors as u64 {
                let mut old = [0xff; SECTOR_SIZE];
                cache.read(disk.as_ref(), sector, &mut old).unwrap();
                assert_eq!(old, [0; SECTOR_SIZE]);
            }
        }

        let waiter = publication_waiter(&fs);
        assert_eq!(waiter.generation(), generation);
        assert_eq!(waiter.result(), None);
        let result = fs.fsync(&file, false);
        let attempts = disk.operations.lock().unwrap();
        let mut expected = vec![Flush];
        if write_limit == 0 {
            for start in [a, b] {
                expected.extend((start..start + sectors as u64).map(|sector| Write {
                    start: sector,
                    sectors: 1,
                }));
            }
        } else {
            expected.extend([ring_a, ring_b]);
        }
        if fail != Some(ring_b) {
            expected.push(Flush);
        }
        assert_eq!(*attempts, expected);
        drop(attempts);

        let durable = disk.recording.0.durable_image();
        if fail.is_none() {
            assert_eq!(result, Ok(()));
            assert_eq!(waiter.result(), Some(Ok(())));
            for start in [a, b] {
                for sector in start..start + sectors as u64 {
                    let offset = sector as usize * SECTOR_SIZE;
                    let mut bytes = [0; SECTOR_SIZE];
                    cache.read(disk.as_ref(), sector, &mut bytes).unwrap();
                    assert_eq!(bytes, durable[offset..offset + SECTOR_SIZE]);
                }
            }
        } else {
            assert_eq!(result, Err(Errno(EIO)));
            assert_eq!(waiter.result(), Some(Err(Error::EIO)));
            assert_eq!(fs.read_only(), Ok(true));
        }
        let recovery = |bytes| {
            recover::recover(
                &Device(Arc::new(RecordingDevice::new(bytes).unwrap())),
                false,
            )
            .unwrap()
        };
        if fail == Some(ring_b) {
            let mut between_rings = durable.clone();
            // Power can fail after ring A reaches the medium but before B.
            // Use the recorder's completed sectors to model that crash image.
            for event in disk.recording.0.events() {
                if let Event::Write { sector, bytes } = event {
                    if (a..a + sectors as u64).contains(&sector) {
                        let offset = sector as usize * SECTOR_SIZE;
                        between_rings[offset..offset + SECTOR_SIZE].copy_from_slice(&bytes);
                    }
                }
            }
            let recovered = recovery(between_rings);
            assert_eq!((recovered.generation, recovered.ring), (generation, 0));
        }
        let recovered = recovery(durable);
        assert_eq!(
            recovered.generation,
            if fail.is_none() {
                generation
            } else {
                previous.generation
            }
        );
    }
}
