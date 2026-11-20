use crate::{
    attrs::InodeAttrs,
    block::{cache::BufferCache, read_only, BlockDevice, BlockError},
    boot_mount_tests::boot::{format_volatile, mount_volatile_if_needed},
    breenfs::vfs::BreenFs,
    errno::Errno,
    intent,
    memory_device::MemoryDevice,
    read_conformance::NoWait,
    statfs_support::{mount, TEST_MOUNTS},
    syscall_errno::EROFS,
    vfs::{FileSystem, NewNode, NodeKind, WriteAt, WriteReq},
};
use breenfs_host_tests::{block::BlockDevice as RecordingIo, recording::RecordingDevice};
use std::sync::Arc;

const ROOT: &str = "01234567-89ab-cdef-1020-304050607080";
const VOLATILE_BYTES: usize = 64 * 1024 * 1024;
const FILE: NewNode<'static> = NewNode::Regular {
    attrs: InodeAttrs::new(0o644, 0, 0),
};

fn root(readonly: bool) -> (usize, Arc<dyn FileSystem>) {
    let fixture = crate::host_mounts::Fixture::new();
    let fixture_fs = crate::adapter::Ext2Adapter::new(fixture.pin());
    for name in [b"tmp", b"run"] {
        fixture_fs
            .create(
                fixture_fs.root().unwrap(),
                name,
                NewNode::Directory {
                    attrs: InodeAttrs::new(0o755, 0, 0),
                },
            )
            .unwrap();
    }
    let disk = fixture.device_image();
    let disk = if readonly {
        read_only::device(disk)
    } else {
        disk
    };
    let fs = crate::host_mounts::mount_root_device(disk).unwrap();
    let id = mount::allocate_id().unwrap();
    let point = crate::format::ExplicitMount {
        id,
        path: "/",
        flags: crate::vfs::MountFlags {
            read_only: readonly,
            ..Default::default()
        },
        cover: None,
        parent: None,
    };
    mount::register(point.entry(fs.clone()).unwrap()).unwrap();
    (id, fs)
}

fn format_three(mode: &str) -> intent::Captured {
    intent::parse(format!("BREENIX-BOOT 3\ntarget default\nmount {ROOT} / {mode}\n").as_bytes())
        .unwrap()
}

#[test]
fn read_only_root_has_writable_breenfs_tmp_and_run() {
    let _registry = TEST_MOUNTS.lock().unwrap();
    let (root_id, root_fs) = root(true);
    let root_ino = root_fs.root().unwrap();

    mount_volatile_if_needed(&format_three("ro"), || {
        Ok(Arc::new(MemoryDevice::new().unwrap()))
    })
    .unwrap();
    assert_eq!(
        mount::mounts_text(),
        b"none / ext2 ro 0 0\nnone /tmp breenfs rw 0 0\nnone /run breenfs rw 0 0\n"
    );

    let mut mounted = Vec::new();
    for (path, mode) in [("/tmp", 0o1777), ("/run", 0o755)] {
        let name = &path.as_bytes()[1..];
        let cover = root_fs.lookup(root_ino, name).unwrap();
        let Some(mount::Entered::Fs(id, dir)) = mount::enter((root_id, cover)) else {
            panic!("{path} has no filesystem cover");
        };
        assert_eq!(mount::covered_parent(id), Some((root_id, root_ino)));
        let fs = mount::filesystem(id).unwrap();
        assert_eq!(fs.kind(), Ok("breenfs"));
        let attr = fs.getattr(dir).unwrap();
        assert_eq!(attr.kind, NodeKind::Directory);
        assert_eq!(attr.attrs.mode & 0o7777, mode);
        let stat = fs.statfs().unwrap();
        assert_eq!(stat.blocks * stat.block_size, VOLATILE_BYTES as u64);
        let ino = fs.create(dir, b"boot-check", FILE).unwrap();
        assert_eq!(fs.lookup(dir, b"boot-check"), Ok(ino));
        assert_eq!(
            root_fs.lookup(cover, b"boot-check"),
            Err(Errno(crate::syscall_errno::ENOENT))
        );
        mounted.push(id);
    }
    assert_eq!(mount::storage_mounts(), [root_id, mounted[0]]);
    assert_eq!(
        root_fs.create(root_ino, b"boot-check", FILE),
        Err(Errno(EROFS))
    );

    for id in mounted.into_iter().rev().chain([root_id]) {
        mount::unmount(id, true).unwrap();
    }
}

#[test]
fn writable_and_legacy_boots_do_not_allocate_volatile_storage() {
    let _registry = TEST_MOUNTS.lock().unwrap();
    let (id, _) = root(false);
    mount_volatile_if_needed(&format_three("rw"), || {
        panic!("allocated a disk for a rw root")
    })
    .unwrap();
    assert_eq!(mount::storage_mounts(), [id]);
    mount::unmount(id, true).unwrap();

    let (id, _) = root(true);
    for payload in [
        "BREENIX-BOOT 1\ndefault\n".to_string(),
        format!("BREENIX-BOOT 2\ntarget default\nroot {ROOT}\n"),
    ] {
        let intent = intent::parse(payload.as_bytes()).unwrap();
        mount_volatile_if_needed(&intent, || panic!("legacy boot allocated a disk")).unwrap();
        assert_eq!(mount::storage_mounts(), [id]);
    }
    mount::unmount(id, true).unwrap();
}

struct RecordedMemory {
    disk: MemoryDevice,
    recording: RecordingDevice,
}

impl BlockDevice for RecordedMemory {
    fn read_block(&self, block: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.disk.read_block(block, out)
    }

    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.disk.write_block(block, bytes)?;
        let mut written = [0; 512];
        self.disk.read_block(block, &mut written)?;
        self.recording.write_block(block, &written).unwrap();
        Ok(())
    }

    fn block_size(&self) -> usize {
        self.disk.block_size()
    }

    fn num_blocks(&self) -> u64 {
        self.disk.num_blocks()
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.disk.flush()?;
        self.recording.flush().unwrap();
        Ok(())
    }
}

#[test]
fn memory_device_obeys_recording_device_publication_order() {
    let disk = MemoryDevice::new().unwrap();
    format_volatile(&disk).unwrap();
    let mut image = vec![0; VOLATILE_BYTES];
    disk.read_blocks(0, disk.num_blocks() as usize, &mut image)
        .unwrap();
    let disk = Arc::new(RecordedMemory {
        disk,
        recording: RecordingDevice::new(image).unwrap(),
    });
    let cache = || Arc::new(BufferCache::new(NoWait));
    let fs = BreenFs::mount_writable(disk.clone(), cache(), 1, 1).unwrap();
    let run = fs.lookup(fs.root().unwrap(), b"run").unwrap();
    let ino = fs.create(run, b"receipt", FILE).unwrap();
    let file = fs.open(ino).unwrap();
    let bytes = b"committed to memory";
    fs.write(
        &file,
        WriteReq {
            at: WriteAt::Offset(0),
            limit: u64::MAX,
            clear_setid: false,
        },
        bytes,
    )
    .unwrap();
    fs.fsync(&file, false).unwrap();
    assert_eq!(disk.recording.durable_generations(), [1, 2]);

    let reader = BreenFs::mount(disk, cache(), 2, 1).unwrap();
    let run = reader.lookup(reader.root().unwrap(), b"run").unwrap();
    let file = reader
        .open(reader.lookup(run, b"receipt").unwrap())
        .unwrap();
    let mut read = [0; 19];
    assert_eq!(reader.read(&file, 0, &mut read), Ok(bytes.len()));
    assert_eq!(&read, bytes);
}
