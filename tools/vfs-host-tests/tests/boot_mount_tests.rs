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
