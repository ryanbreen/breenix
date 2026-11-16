//! The 24.8 oracle and streams against the production FileSystem boundary.
use breenfs_host_tests::{compare, model::Model, recording::RecordingDevice, stream::Stream, *};
use std::sync::Arc;

struct TraitAdapter;
impl TraitAdapter {
    fn new(_device: Arc<RecordingDevice>) -> Self {
        Self
    }
}
impl Fs for TraitAdapter {
    fn execute(&mut self, _op: &Operation) -> Result<Reply> {
        // Check-first stub: the trait adapter is not implemented yet.
        let unsupported: core::result::Result<Reply, crate::errno::Errno> =
            Err(crate::errno::Errno(crate::syscall_errno::EOPNOTSUPP));
        unsupported.map_err(|_| Error::Unimplemented)
    }
    fn state(&self) -> Result<State> {
        Err(Error::Unimplemented)
    }
}

fn image() -> Vec<u8> {
    use crate::{
        breenfs::{
            commit::{Committer, DeviceStore},
            format::Superblock,
            ops::Files,
        },
        read_conformance::Disk,
    };
    use std::sync::Mutex;
    let disk = Disk {
        bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
        writable: true,
    };
    let sb = Superblock {
        fs_uuid: [0x73; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        global_reserve: 4096,
        next_root_id: 2,
        dirhash_key: [0x31; 16],
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    files.fsync(&disk, &mut Committer::format(sb)).unwrap();
    drop(files);
    disk.bytes.into_inner().unwrap()
}

#[test]
fn trait_seeds_1_to_64_match_the_model() {
    let initial = image();
    let mut covered = std::collections::HashSet::new();
    for seed in 1..=64 {
        // RecordingDevice enforces W1-W3 on every write, including remount recovery.
        let device = Arc::new(RecordingDevice::new(initial.clone()).unwrap());
        let mut fs = TraitAdapter::new(device);
        let mut model = Model::new();
        for (index, op) in Stream::new(seed).take(256).enumerate() {
            covered.insert(std::mem::discriminant(&op.action));
            let expected = model.execute(&op);
            assert_eq!(fs.execute(&op), expected, "seed={seed} op={index} {op:?}");
            if op.action == Action::Remount {
                let diff = compare(&model, &fs).unwrap();
                assert!(diff.is_empty(), "seed={seed} op={index} {op:?}: {diff:?}");
            }
        }
        let op = Operation {
            at: Time::default(),
            action: Action::Remount,
        };
        assert_eq!(
            fs.execute(&op),
            model.execute(&op),
            "seed={seed} final remount"
        );
        let diff = compare(&model, &fs).unwrap();
        assert!(diff.is_empty(), "seed={seed} final remount: {diff:?}");
    }
    assert_eq!(
        covered.len(),
        14,
        "every stream operation must be exercised"
    );
}
