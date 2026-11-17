//! The 24.8 oracle and streams against the production FileSystem boundary.
use breenfs_host_tests::{compare, model::Model, recording::RecordingDevice, stream::Stream, *};
use std::sync::Arc;

use crate::trait_adapter::TraitAdapter;

pub(crate) fn image() -> Vec<u8> {
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
        // Writes and remount recovery pass through the W1-W3 recording device.
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

#[test]
fn model_and_writer_use_linux_removal_errnos() {
    fn errors(fs: &mut impl Fs) -> [Result<Reply>; 2] {
        let op = |action| Operation {
            at: Time::default(),
            action,
        };
        fs.execute(&op(Action::Mkdir {
            dir: ROOT,
            name: b"dir".to_vec(),
            mode: 0o755,
        }))
        .unwrap();
        [
            fs.execute(&op(Action::Unlink {
                dir: ROOT,
                name: b"dir".to_vec(),
            })),
            fs.execute(&op(Action::Removexattr {
                ino: ROOT,
                name: b"user.absent".to_vec(),
            })),
        ]
    }
    let device = Arc::new(RecordingDevice::new(image()).unwrap());
    let mut writer = WriterAdapter::new(device).unwrap();
    let mut model = Model::new();
    let expected = [Err(Errno::EISDIR.into()), Err(Errno::ENODATA.into())];
    assert_eq!([errors(&mut model), errors(&mut writer)], [expected; 2]);
}

#[test]
fn minimized_seed_1_closed_orphan_is_absent() {
    // Seed 1, minimized by deleting operations until no single deletion kept
    // the truncate-after-close divergence: create, sparse write, unlink, truncate.
    let device = Arc::new(RecordingDevice::new(image()).unwrap());
    let mut fs = TraitAdapter::new(device);
    let mut model = Model::new();
    for (index, op) in Stream::new(1).take(15).enumerate() {
        if [0, 1, 10, 14].contains(&index) {
            assert_eq!(
                fs.execute(&op),
                model.execute(&op),
                "seed=1 op={index} {op:?}"
            );
        }
    }
}

#[test]
fn minimized_seed_2_same_size_truncate_preserves_times() {
    // Deletion-minimized seed 2: retain the allocations that give the target
    // inode 26, then truncate it to 4095 twice at distinct logical times.
    let keep = [0, 15, 24, 28, 32, 78, 80, 97, 100, 108, 110, 124, 152];
    let device = Arc::new(RecordingDevice::new(image()).unwrap());
    let mut fs = TraitAdapter::new(device);
    let mut model = Model::new();
    for (index, op) in Stream::new(2).take(153).enumerate() {
        if keep.contains(&index) {
            assert_eq!(fs.execute(&op), model.execute(&op), "seed=2 op={index} {op:?}");
        }
    }
    let remount = Operation {
        at: Time::default(),
        action: Action::Remount,
    };
    assert_eq!(fs.execute(&remount), model.execute(&remount));
    let diff = compare(&model, &fs).unwrap();
    assert!(diff.is_empty(), "seed=2 keep={keep:?}: {diff:?}");
}
