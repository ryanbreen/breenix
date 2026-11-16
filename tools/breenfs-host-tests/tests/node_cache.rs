use breenfs_host_tests::{
    block::{BlockDevice, BlockError},
    btree::{self, NodeStore},
    buffer_cache::{BufferCache, Wait},
    crc32c,
    format::{BlockPtr, Key, Superblock, BLOCK_SIZE, NODE_SIZE},
    node_cache::CachedNodeStore,
    recover,
};
use std::{
    fs,
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("sequential node reads must not wait")
    }
    fn wake_all(&self) {}
}

struct Device {
    bytes: Mutex<Vec<u8>>,
    sector: usize,
    reads: AtomicU64,
}
impl Device {
    fn new(sector: usize) -> Self {
        Self {
            bytes: Mutex::new(populated_image()),
            sector,
            reads: AtomicU64::new(0),
        }
    }
}
impl BlockDevice for Device {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let bytes = self.bytes.lock().unwrap();
        let start = usize::try_from(block)
            .ok()
            .and_then(|b| b.checked_mul(self.sector))
            .ok_or(BlockError::OutOfBounds)?;
        let source = bytes
            .get(start..start + self.sector)
            .ok_or(BlockError::OutOfBounds)?;
        assert_eq!(buf.len(), self.sector);
        buf.copy_from_slice(source);
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn write_block(&self, _: u64, _: &[u8]) -> Result<(), BlockError> {
        panic!("read-only node store wrote the device")
    }
    fn block_size(&self) -> usize {
        self.sector
    }
    fn num_blocks(&self) -> u64 {
        (self.bytes.lock().unwrap().len() / self.sector) as u64
    }
    fn flush(&self) -> Result<(), BlockError> {
        panic!("read-only node store flushed the device")
    }
}

fn populated_image() -> Vec<u8> {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let dir =
                std::env::temp_dir().join(format!("breenfs-node-cache-{}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let source = dir.join("source");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("file"), b"verified node reads").unwrap();
            let image = dir.join("populated.img");
            let output = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-mkfs"))
                .args([
                    "--size",
                    "67108864",
                    "--time",
                    "0",
                    "--uuid",
                    "00112233445566778899aabbccddeeff",
                    "--hash-key",
                    "000102030405060708090a0b0c0d0e0f",
                    "--populate",
                ])
                .arg(&source)
                .arg(&image)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let bytes = fs::read(image).unwrap();
            fs::remove_dir_all(dir).unwrap();
            bytes
        })
        .clone()
}

fn root_bytes(device: &Device, sb: &Superblock) -> Vec<u8> {
    let start = sb.space_root.addr as usize * BLOCK_SIZE;
    device.bytes.lock().unwrap()[start..start + NODE_SIZE].to_vec()
}

#[test]
fn clean_node_is_verified_then_cached() {
    for sector in [512, BLOCK_SIZE] {
        let device = Device::new(sector);
        let sb = recover::recover(&device, false).unwrap();
        let cache = BufferCache::new(NoWait);
        let mut store = CachedNodeStore::new(&cache, &device, &sb);
        let before = device.reads.load(Ordering::Relaxed);
        let expected = root_bytes(&device, &sb);
        assert_eq!(store.get(sb.space_root.addr).unwrap(), expected);
        let reads = cache.counters().device_reads.count;
        assert!(reads > 0);
        assert_eq!(
            device.reads.load(Ordering::Relaxed) - before,
            (NODE_SIZE / sector) as u64
        );
        let native_reads = device.reads.load(Ordering::Relaxed);
        assert_eq!(store.get(sb.space_root.addr).unwrap(), expected);
        assert_eq!(cache.counters().device_reads.count, reads);
        assert_eq!(device.reads.load(Ordering::Relaxed), native_reads);
        assert_eq!(cache.counters().hits.count, 1);
        assert_eq!(
            store.put(sb.space_root.addr, expected),
            Err(btree::Error::Store)
        );
        assert_eq!(store.alloc(), Err(btree::Error::Store));

        // Warm the decoded cache before trying mismatched parent pointers.
        let found = btree::search(&store, sb.space_root, Key::default()).unwrap();
        let before = cache.counters();
        assert_eq!(btree::search(&store, sb.space_root, Key::default()).unwrap(), found);
        assert_eq!(cache.counters(), before, "decoded hits must not reread node bytes");
        // Cache admission is mount-wide; exact parent-pointer checks remain in decode.
        for ptr in [
            BlockPtr {
                generation: sb.space_root.generation + 1,
                ..sb.space_root
            },
            BlockPtr {
                csum: sb.space_root.csum ^ 1,
                ..sb.space_root
            },
            BlockPtr {
                level: sb.space_root.level + 1,
                ..sb.space_root
            },
        ] {
            assert_eq!(
                btree::search(&store, ptr, Key::default()),
                Err(btree::Error::InvalidNode)
            );
        }
    }
}

#[test]
fn invalid_nodes_are_refused_and_retried_from_device() {
    let device = Device::new(512);
    let sb = recover::recover(&device, false).unwrap();
    let pristine = root_bytes(&device, &sb);
    for fault in [
        "crc",
        "addr",
        "zero-generation",
        "future-generation",
        "uuid",
        "last-block",
    ] {
        let mut node = pristine.clone();
        match fault {
            "crc" => node[8] ^= 1,
            "addr" => node[32..40].copy_from_slice(&(sb.space_root.addr + 4).to_le_bytes()),
            "zero-generation" => node[40..48].fill(0),
            "future-generation" => node[40..48].copy_from_slice(&(sb.generation + 1).to_le_bytes()),
            "uuid" => node[16] ^= 1,
            "last-block" => node[NODE_SIZE - 1] ^= 1,
            _ => unreachable!(),
        }
        if !matches!(fault, "crc" | "last-block") {
            let csum = crc32c::checksum(&node[12..]);
            node[8..12].copy_from_slice(&csum.to_le_bytes());
        }
        let start = sb.space_root.addr as usize * BLOCK_SIZE;
        device.bytes.lock().unwrap()[start..start + NODE_SIZE].copy_from_slice(&node);
        let cache = BufferCache::new(NoWait);
        let store = CachedNodeStore::new(&cache, &device, &sb);
        for attempt in 1..=2 {
            let before = device.reads.load(Ordering::Relaxed);
            assert_eq!(
                store.get(sb.space_root.addr),
                Err(btree::Error::Store),
                "{fault}"
            );
            assert_eq!(cache.counters().device_reads.count, attempt, "{fault}");
            assert_eq!(
                device.reads.load(Ordering::Relaxed) - before,
                (NODE_SIZE / 512) as u64,
                "{fault}"
            );
        }
        device.bytes.lock().unwrap()[start..start + NODE_SIZE].copy_from_slice(&pristine);
        assert_eq!(
            store.get(sb.space_root.addr).unwrap(),
            pristine,
            "{fault} repair"
        );
        assert_eq!(cache.counters().device_reads.count, 3, "{fault} repair");
    }
}
