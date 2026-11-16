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
