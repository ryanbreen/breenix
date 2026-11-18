//! E24 ledger 509(n), #3064: the acceptance-m namespace admission probe.
//! The default root has the replay's geometry, FS/SPACE heights and about 1000
//! populated inodes. ROOT_REPLAY_IMAGE optionally selects the retained capture.
use super::*;
use std::sync::Mutex as HostMutex;
struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("unexpected sequential wait")
    }
    fn wake_all(&self) {}
}
struct Disk(HostMutex<Vec<u8>>);
impl BlockDevice for Disk {
    fn block_size(&self) -> usize {
        512
    }
    fn num_blocks(&self) -> u64 {
        self.0.lock().unwrap().len() as u64 / 512
    }
    fn read_block(&self, b: u64, out: &mut [u8]) -> Result<(), BlockError> {
        let a = b as usize * 512;
        out.copy_from_slice(&self.0.lock().unwrap()[a..a + out.len()]);
        Ok(())
    }
    fn write_block(&self, b: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let a = b as usize * 512;
        self.0.lock().unwrap()[a..a + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}
fn populated_root() -> Vec<u8> {
    if let Ok(path) = std::env::var("ROOT_REPLAY_IMAGE") {
        return std::fs::read(path).unwrap();
    }
    let disk = Disk(HostMutex::new(vec![0; 73472 * BLOCK_SIZE]));
    let sb = Superblock {
        fs_uuid: [0x15; 16],
        generation: 1,
        slot: 1,
        total_blocks: 73472,
        global_reserve: 4096,
        next_root_id: 2,
        dirhash_key: [0x31; 16],
        ..Superblock::default()
    };
    let at = Time {
        seconds: 1,
        nanos: 0,
    };
    {
        let mut files = Files::format(super::super::commit::DeviceStore(&disk), &sb).unwrap();
        files.mkdir(2, b"tmp", 0o1777, at).unwrap();
        let bytes = vec![0x61; 51 * BLOCK_SIZE];
        for i in 0..994 {
            let ino = files
                .create(2, alloc::format!("populated-{i}").as_bytes(), 0o644, at)
                .unwrap();
            files.write(&disk, ino, 0, &bytes, at).unwrap();
        }
        files.fsync(&disk, &mut Committer::format(sb)).unwrap();
    }
    disk.0.into_inner().unwrap()
}
fn fixture() -> Arc<BreenFs<NoWait>> {
    let disk = Arc::new(Disk(HostMutex::new(populated_root())));
    BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 241515, 1).unwrap()
}
#[test]
fn namespace_churn_before_periodic_commit_must_not_refuse_admission() {
    let fs = fixture();
    let tmp = fs.lookup(2, b"tmp").unwrap();
    let mut refused = None;
    let first_estimate = {
        let mut state = fs.inner.state.lock();
        let grant = state
            .reserve_metadata(&[tmp], 5, true, false, Operation::Allocate)
            .unwrap();
        let blocks = grant.blocks();
        state.reservations.release(grant);
        blocks
    };
    for i in 0..64 {
        let name = alloc::format!("acceptance-churn-{i}");
        let result = fs.create(
            tmp,
            name.as_bytes(),
            NewNode::Directory {
                attrs: InodeAttrs::new(0o777, 0, 0),
            },
        );
        if let Err(error) = result {
            let s = fs.inner.state.lock();
            // Refusal diagnostic only. Since 597/601 no prepared view exists
            // before install; admission reads the published view (durable).
            let store = fs.inner.cache.store(fs.inner.device.clone(), &s.durable);
            let promotion =
                super::super::space::pending_promotion_generation(&store, s.durable.space_root);
            std::eprintln!("REFUSAL i={i} error={error:?} generation={} free={} reserve={} outstanding={} available={} pending_promotion={promotion:?} failed={}",s.durable.generation,s.durable.free_blocks,s.durable.global_reserve,s.reservations.outstanding(),s.reservations.available(s.durable.free_blocks),s.failed);
            refused = Some((i, name));
            break;
        }
        let ino = result.unwrap();
        if i == 0 {
            let retained = fs.inner.state.lock().reservations.outstanding();
            println!("METADATA_SETTLEMENT granted={first_estimate} retained={retained}");
            assert!(
                retained < first_estimate,
                "unused worst-case grant was not returned at completion"
            );
        }
        fs.remove(
            tmp,
            name.as_bytes(),
            Remove {
                directory: true,
                expected: ino,
                owner: 0,
                parent: fs.getattr(tmp).unwrap().attrs,
            },
        )
        .unwrap();
    }
    if let Some((i, name)) = &refused {
        fs.fsync(&fs.open(2).unwrap(), false).unwrap();
        let result = fs.create(
            tmp,
            name.as_bytes(),
            NewNode::Directory {
                attrs: InodeAttrs::new(0o777, 0, 0),
            },
        );
        let s = fs.inner.state.lock();
        std::eprintln!(
            "AFTER EXPLICIT FSYNC i={i} retry={result:?} generation={} free={} outstanding={}",
            s.durable.generation,
            s.durable.free_blocks,
            s.reservations.outstanding()
        );
        assert!(result.is_ok(), "explicit publication control failed");
    }
    // Keep the explicit-fsync control exercised on green code too.
    fs.fsync(&fs.open(2).unwrap(), false).unwrap();
    let control = fs.create(
        tmp,
        b"explicit-fsync-control",
        NewNode::Directory {
            attrs: InodeAttrs::new(0o777, 0, 0),
        },
    );
    assert!(control.is_ok(), "explicit publication control failed");
    assert!(
        refused.is_none(),
        "namespace admission refused until explicit publication"
    );
}

#[test]
fn retained_grants_publish_before_admission_refuses_without_pending_ranges() {
    let fs = fixture();
    fs.create(
        2,
        b"retained",
        NewNode::Directory {
            attrs: InodeAttrs::new(0o755, 0, 0),
        },
    )
    .unwrap();
    let state = fs.inner.state.lock();
    let generation = state.durable.generation;
    assert!(!state.grants.is_empty());
    let store = fs
        .inner
        .cache
        .store(fs.inner.device.clone(), &state.durable);
    assert_eq!(
        super::super::space::pending_promotion_generation(&store, state.durable.space_root)
            .unwrap(),
        None
    );
    // Request just beyond current availability, but within the durable capacity.
    // This is the same ledger admission the namespace/data estimate calls.
    let blocks = state.reservations.available(state.durable.free_blocks) + 1;
    assert!(blocks <= state.durable.free_blocks - state.durable.global_reserve);
    let admitted = fs.reserve_with_pressure(
        super::CostOp::Create, state,
        || Ok(fs.inner.state.lock()),
        |state| {
            state
                .reservations
                .reserve(state.durable.free_blocks, blocks, Operation::Allocate)
                .map_err(|_| Errno(ENOSPC))
        },
    );
    assert!(
        admitted.is_ok(),
        "retained open-transaction grants must be published before refusal"
    );
    let (mut state, grant) = admitted.unwrap();
    assert!(state.durable.generation > generation);
    state.reservations.release(grant);
    assert!(!state.failed);
}
