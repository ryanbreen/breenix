use crate::{errno::Errno, syscall_errno::*};

#[test]
fn probe_mounts_ext2_and_refuses_zeroed_disk() {
    use crate::format::{probe_mount, BootMount};
    let disk = crate::host_mounts::Fixture::new();
    let device = disk.device_image();
    let fs = probe_mount(&[crate::ext2_format::FORMAT], device, BootMount::Root).unwrap();
    assert_eq!(fs.kind().unwrap(), "ext2");
    let root = fs.root().unwrap();
    assert_eq!(
        fs.getattr(root).unwrap().kind,
        crate::vfs::NodeKind::Directory
    );
    let ino = fs
        .create(
            root,
            b"probed",
            crate::vfs::NewNode::Regular {
                attrs: crate::attrs::InodeAttrs::new(0o640, 0, 0),
            },
        )
        .unwrap();
    let node = fs.open(ino).unwrap();
    fs.write(
        &node,
        crate::vfs::WriteReq {
            at: crate::vfs::WriteAt::Offset(0),
            limit: u64::MAX,
            clear_setid: false,
        },
        b"magic selected mount",
    )
    .unwrap();
    let mut bytes = [0; 20];
    assert_eq!(fs.read(&node, 0, &mut bytes).unwrap(), 20);
    assert_eq!(&bytes, b"magic selected mount");
    let zero = crate::host_mounts::zeroed_device();
    assert!(matches!(
        probe_mount(&[crate::ext2_format::FORMAT], zero, BootMount::Home),
        Err(Errno(ENODEV))
    ));
}

// Exercise the boot planner on real superblocks and a GPT-backed disk. The
// host publishes the selected filesystems to the same registry as /proc/mounts.
#[path = "../../../kernel/src/fs/vfs/boot.rs"]
pub(crate) mod boot;
#[path = "../../../kernel/src/block/boot_mount.rs"]
mod selection;

use crate::{
    block::{cache::BufferCache, write_policy::Fence, BlockDevice, BlockError, HardwareDisk},
    breenfs::{
        commit::{Committer, DeviceStore},
        format::Superblock,
        ops::Files,
        probe as breenfs_probe,
        vfs::BreenFs,
    },
    format::{BootMount, Format},
    gpt, intent, partition,
    statfs_support::mount,
    vfs::NewNode,
};
use std::sync::Arc;

const TEST: &str = "01234567-89ab-cdef-1020-304050607080";
const SYSTEM: &str = "11234567-89ab-cdef-1020-304050607080";
const HOME: &str = "21234567-89ab-cdef-1020-304050607080";
const NEW_SYSTEM: &str = "31234567-89ab-cdef-1020-304050607080";
const NEW_HOME: &str = "41234567-89ab-cdef-1020-304050607080";
const ABSENT: &str = "51234567-89ab-cdef-1020-304050607080";

const MIGRATION: &[(&str, &str, bool)] = &[
    (SYSTEM, "/migrate/ext2-system", false),
    (HOME, "/migrate/ext2-home", false),
    (TEST, "/", false),
    (NEW_SYSTEM, "/migrate/system", true),
    (NEW_HOME, "/migrate/home", true),
];
const CUTOVER: &[(&str, &str, bool)] = &[(NEW_SYSTEM, "/", true), (NEW_HOME, "/home", true)];
const RESCUE: &[(&str, &str, bool)] = &[(SYSTEM, "/", false), (HOME, "/home", false)];

fn captured(lines: &[(&str, &str, bool)]) -> intent::Captured {
    let mut payload = String::from("BREENIX-BOOT 3\ntarget default\n");
    for (guid, path, rw) in lines {
        payload.push_str(&format!(
            "mount {guid} {path} {}\n",
            if *rw { "rw" } else { "ro" }
        ));
    }
    intent::parse(payload.as_bytes()).unwrap()
}

fn uuid(guid: &str) -> [u8; 16] {
    let bytes: String = guid.chars().filter(|&ch| ch != '-').collect();
    core::array::from_fn(|i| u8::from_str_radix(&bytes[i * 2..i * 2 + 2], 16).unwrap())
}

fn ext2(guid: &str) -> Box<dyn BlockDevice> {
    let fixture = crate::host_mounts::Fixture::new();
    let device = fixture.device_image();
    let mut sector = [0; 512];
    device.read_block(2, &mut sector).unwrap();
    sector[104..120].copy_from_slice(&uuid(guid));
    device.write_block(2, &sector).unwrap();
    device
}

const BREENFS_BLOCKS: u64 = 16_384;
const BREENFS_SECTORS: u64 =
    BREENFS_BLOCKS * (crate::breenfs::format::BLOCK_SIZE / gpt::SECTOR) as u64;

#[derive(Default)]
struct SparseBreenFsDisk(std::sync::Mutex<std::collections::BTreeMap<u64, [u8; gpt::SECTOR]>>);

impl BlockDevice for SparseBreenFsDisk {
    fn block_size(&self) -> usize {
        gpt::SECTOR
    }

    fn num_blocks(&self) -> u64 {
        BREENFS_SECTORS
    }

    fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if sector >= self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }
        if let Some(data) = self.0.lock().unwrap().get(&sector) {
            buf.copy_from_slice(data);
        } else {
            buf.fill(0);
        }
        Ok(())
    }

    fn write_block(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        if sector >= self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }
        self.0.lock().unwrap().insert(sector, buf.try_into().unwrap());
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}

fn breenfs(guid: &str) -> Box<dyn BlockDevice> {
    let disk = Box::new(SparseBreenFsDisk::default());
    let sb = Superblock {
        fs_uuid: uuid(guid),
        generation: 1,
        slot: 1,
        total_blocks: BREENFS_BLOCKS,
        global_reserve: 32,
        next_root_id: 2,
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    files
        .fsync(disk.as_ref(), &mut Committer::format(sb))
        .unwrap();
    disk
}

fn formats() -> [Format; 2] {
    [
        crate::ext2_format::FORMAT,
        Format {
            kind: "breenfs",
            probe: breenfs_probe::probe,
            uuid: breenfs_probe::uuid,
            mount: |device, target| {
                let BootMount::Explicit(point) = target else {
                    unreachable!()
                };
                let cache = Arc::new(BufferCache::new(crate::read_conformance::NoWait));
                let fs = if point.flags.read_only {
                    BreenFs::mount(device.into(), cache, point.id, 1)?
                } else {
                    BreenFs::mount_writable(device.into(), cache, point.id, 1)?
                };
                Ok(fs)
            },
        },
    ]
}

fn arm_disks() -> Vec<Box<dyn BlockDevice>> {
    vec![
        breenfs(NEW_HOME),
        ext2(HOME),
        ext2(TEST),
        breenfs(NEW_SYSTEM),
        ext2(SYSTEM),
    ]
}

fn arm_plan(
    intent: &intent::Captured,
) -> Result<Option<Vec<boot::MountLine<'_>>>, boot::MountFailure<'_>> {
    boot::prepare_mounts(Some(intent), &formats(), |guid, _, _| {
        selection::find_filesystem(arm_disks(), &formats(), guid)
    })
}

#[test]
fn format_three_mounts_migration_cutover_and_rescue_by_uuid() {
    let _registry = crate::statfs_support::TEST_MOUNTS.lock().unwrap();
    for (lines, expected) in [
        (
            MIGRATION,
            concat!(
                "none / ext2 ro 0 0\n",
                "none /migrate/ext2-system ext2 ro 0 0\n",
                "none /migrate/ext2-home ext2 ro 0 0\n",
                "none /migrate/system breenfs rw 0 0\n",
                "none /migrate/home breenfs rw 0 0\n",
            ),
        ),
        (
            CUTOVER,
            "none / breenfs rw 0 0\nnone /home breenfs rw,nosuid,nodev 0 0\n",
        ),
        (
            RESCUE,
            "none / ext2 ro 0 0\nnone /home ext2 ro,nosuid,nodev 0 0\n",
        ),
    ] {
        let intent = captured(lines);
        let plan = arm_plan(&intent).unwrap().unwrap();
        let mut ids = Vec::new();
        for line in plan {
            let point = line.point(mount::allocate_id().unwrap(), None, None);
            let fs = line.mount(&formats(), &point).unwrap();
            assert_eq!(fs.read_only().unwrap(), point.flags.read_only);
            let result = fs.create(
                fs.root().unwrap(),
                b"boot-check",
                NewNode::Directory {
                    attrs: crate::attrs::InodeAttrs::new(0o755, 0, 0),
                },
            );
            if point.flags.read_only {
                assert_eq!(result, Err(Errno(EROFS)));
            } else {
                result.unwrap();
            }
            ids.push(point.id);
            mount::register(point.entry(fs).unwrap()).unwrap();
        }
        assert_eq!(mount::mounts_text(), expected.as_bytes());
        for id in ids.into_iter().rev() {
            mount::unmount(id, true).unwrap();
        }
    }
}

struct GptDisk {
    header: [u8; 512],
    entries: [u8; 1024],
    images: Vec<(u64, Box<dyn BlockDevice>)>,
    hardware: HardwareDisk,
}

const PARTITION_SECTORS: u64 = 4096;
const EXT2_HOME_START: u64 = 3 * PARTITION_SECTORS;
const BREENFS_SYSTEM_START: u64 = EXT2_HOME_START + PARTITION_SECTORS;
const BREENFS_HOME_START: u64 = BREENFS_SYSTEM_START + BREENFS_SECTORS;
const PARTITIONS_END: u64 = BREENFS_HOME_START + BREENFS_SECTORS;

impl GptDisk {
    fn new(hardware: HardwareDisk) -> Self {
        let roles = [
            (gpt::TEST_ROOT_TYPE, TEST, ext2(TEST)),
            (gpt::SYSTEM_TYPE, SYSTEM, ext2(SYSTEM)),
            (gpt::HOME_TYPE, HOME, ext2(HOME)),
            (
                selection::BREENFS_SYSTEM_TYPE,
                NEW_SYSTEM,
                breenfs(NEW_SYSTEM),
            ),
            (selection::BREENFS_HOME_TYPE, NEW_HOME, breenfs(NEW_HOME)),
        ];
        let mut entries = [0; 1024];
        let mut images = Vec::new();
        let mut first = PARTITION_SECTORS;
        for (i, (role, guid, image)) in roles.into_iter().enumerate() {
            let entry = &mut entries[i * 128..(i + 1) * 128];
            entry[..16].copy_from_slice(&role);
            entry[16..32].copy_from_slice(&gpt::parse_guid(guid).unwrap());
            let end = first + image.num_blocks();
            entry[32..40].copy_from_slice(&first.to_le_bytes());
            entry[40..48].copy_from_slice(&(end - 1).to_le_bytes());
            images.push((first, image));
            first = end;
        }
        let mut header = [0; 512];
        header[..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        header[12..16].copy_from_slice(&92u32.to_le_bytes());
        header[24..32].copy_from_slice(&1u64.to_le_bytes());
        header[40..48].copy_from_slice(&PARTITION_SECTORS.to_le_bytes());
        header[48..56].copy_from_slice(&(PARTITIONS_END - 1).to_le_bytes());
        header[72..80].copy_from_slice(&2u64.to_le_bytes());
        header[80..84].copy_from_slice(&8u32.to_le_bytes());
        header[84..88].copy_from_slice(&128u32.to_le_bytes());
        header[88..92].copy_from_slice(&gpt::crc32(&entries).to_le_bytes());
        let crc = gpt::crc32(&header[..92]);
        header[16..20].copy_from_slice(&crc.to_le_bytes());
        Self {
            header,
            entries,
            images,
            hardware,
        }
    }
}

impl BlockDevice for GptDisk {
    fn hardware_disk(&self) -> Option<HardwareDisk> {
        Some(self.hardware)
    }
    fn block_size(&self) -> usize {
        512
    }
    fn num_blocks(&self) -> u64 {
        PARTITIONS_END + PARTITION_SECTORS
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        match block {
            1 => buf.copy_from_slice(&self.header),
            2 => buf.copy_from_slice(&self.entries[..512]),
            3 => buf.copy_from_slice(&self.entries[512..]),
            n => {
                for (first, image) in &self.images {
                    if (*first..*first + image.num_blocks()).contains(&n) {
                        return image.read_block(n - first, buf);
                    }
                }
                buf.fill(0);
            }
        }
        Ok(())
    }
    fn write_block(&self, _: u64, _: &[u8]) -> Result<(), BlockError> {
        panic!("boot planning must not write to the disk")
    }
}

#[test]
fn format_three_x86_fences_only_the_writable_partitions() {
    let hardware = HardwareDisk::Ahci(0);
    for (lines, expected) in [
        (
            MIGRATION,
            &[
                (BREENFS_SYSTEM_START, BREENFS_HOME_START - 1),
                (BREENFS_HOME_START, PARTITIONS_END - 1),
            ][..],
        ),
        (
            CUTOVER,
            &[
                (BREENFS_SYSTEM_START, BREENFS_HOME_START - 1),
                (BREENFS_HOME_START, PARTITIONS_END - 1),
            ][..],
        ),
        (RESCUE, &[][..]),
    ] {
        let intent = captured(lines);
        let mut fence = Fence::CLOSED;
        let plan = boot::prepare_mounts(Some(&intent), &formats(), |guid, path, rw| {
            let (disk, entry, hardware) = selection::find_partition(
                vec![Box::new(GptDisk::new(hardware)) as Box<dyn BlockDevice>],
                guid,
                selection::partition_types(path),
            )?;
            selection::grant_write(&mut fence, hardware, &entry, rw)?;
            Ok(Box::new(disk))
        })
        .unwrap()
        .unwrap();
        assert_eq!(plan.len(), lines.len());
        assert_eq!(fence.ranges(), expected);
        assert!(!fence.allows(hardware, EXT2_HOME_START, 1));
        assert_eq!(
            fence.allows(hardware, BREENFS_SYSTEM_START, 1),
            !expected.is_empty()
        );
        assert!(!fence.allows(HardwareDisk::Ahci(1), BREENFS_SYSTEM_START, 1));
    }
}

#[test]
fn format_three_refuses_absent_ambiguous_wrong_role_and_third_write() {
    for (guid, duplicate, reason) in [
        (ABSENT, false, "no attached"),
        (TEST, true, "more than one"),
    ] {
        let intent = captured(&[(guid, "/", false)]);
        let error = boot::prepare_mounts(Some(&intent), &formats(), |guid, _, _| {
            let mut disks = arm_disks();
            if duplicate {
                disks.push(ext2(TEST));
            }
            selection::find_filesystem(disks, &formats(), guid)
        })
        .err()
        .expect("UUID refused");
        assert!(error.to_string().contains(reason));
        assert!(error.to_string().contains(guid));
    }

    let hardware = HardwareDisk::Ahci(0);
    let find = |guid, path, duplicate| {
        let mut disks: Vec<Box<dyn BlockDevice>> = vec![Box::new(GptDisk::new(hardware))];
        if duplicate {
            disks.push(Box::new(GptDisk::new(HardwareDisk::Ahci(1))));
        }
        selection::find_partition(disks, guid, selection::partition_types(path))
            .err()
            .unwrap()
    };
    assert!(find(ABSENT, "/", false).contains("no attached"));
    assert!(find(TEST, "/", true).contains("more than one"));
    assert!(find(NEW_HOME, "/", false).contains("type"));

    let intent = captured(&[
        (NEW_SYSTEM, "/", true),
        (NEW_HOME, "/home", true),
        (TEST, "/tmp", true),
    ]);
    let failure = boot::prepare_mounts(Some(&intent), &formats(), |_, _, _| {
        panic!("invalid intent reached discovery")
    })
    .err()
    .expect("third writable line refused");
    assert!(failure.to_string().contains("too many writable mounts"));
}

#[test]
fn format_one_and_two_keep_legacy_mount_dispatch() {
    for payload in [
        "BREENIX-BOOT 1\ndefault\n".to_string(),
        format!("BREENIX-BOOT 2\ntarget default\nroot {TEST}\nhome {SYSTEM}\n"),
    ] {
        let intent = intent::parse(payload.as_bytes()).unwrap();
        assert!(
            boot::prepare_mounts(Some(&intent), &formats(), |_, _, _| panic!(
                "legacy intent used explicit discovery"
            ))
            .unwrap()
            .is_none()
        );
        for target in [BootMount::Root, BootMount::Home] {
            let fs = crate::format::probe_mount(&formats(), ext2(TEST), target).unwrap();
            assert_eq!(fs.kind(), Ok("ext2"));
            assert_eq!(fs.read_only(), Ok(false));
        }
    }
}
