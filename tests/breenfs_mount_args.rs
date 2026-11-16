//! Pure mount validation, independent of devices, namespaces and user pointers.
#[path = "../kernel/src/syscall/errno.rs"]
pub mod errno;
pub mod syscall {
    pub use crate::errno;
}
#[path = "../kernel/src/fs/breenfs/mount_args.rs"]
mod mount_args;

#[test]
fn readonly_disk_mount_arguments_and_magic() {
    use errno::{EINVAL, EPERM, EROFS};
    let cases: &[(u32, u64, &[u8], Result<usize, u64>)] = &[
        (0, 1, b"/dev/disk0", Ok(0)),
        (0, 1, b"/dev/disk9", Ok(9)),
        (0, 1, b"/dev/disk10", Ok(10)),
        (0, 1, b"/dev/disk255", Ok(255)),
        (1000, 1, b"/dev/disk0", Err(EPERM as u64)),
        (1000, 0, b"", Err(EPERM as u64)),
        (0, 0, b"/dev/disk0", Err(EROFS as u64)),
        (0, 2, b"/dev/disk0", Err(EINVAL as u64)),
        (0, 3, b"/dev/disk0", Err(EINVAL as u64)),
        (0, 1 << 63, b"/dev/disk0", Err(EINVAL as u64)),
        (0, 1 | (1 << 63), b"/dev/disk0", Err(EINVAL as u64)),
        (0, 1, b"", Err(EINVAL as u64)),
        (0, 1, b"disk0", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk00", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk01", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk+1", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk-1", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk256", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk99999999999999999999", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk1/", Err(EINVAL as u64)),
        (0, 1, b"/dev/disk1\0", Err(EINVAL as u64)),
        (0, 1, b"/dev/null", Err(EINVAL as u64)),
    ];
    for &(euid, flags, source, expected) in cases {
        assert_eq!(mount_args::parse(euid, flags, source), expected,
            "euid={euid} flags={flags:#x} source={source:?}");
    }
    for bit in 1..64 {
        assert_eq!(mount_args::parse(0, 1 | (1 << bit), b"/dev/disk0"), Err(EINVAL as u64));
    }
    for (bytes, expected) in [
        (&b"BREENFSS"[..], true),
        (&b"BREENFSSrest"[..], true),
        (&b"BREENFSN"[..], false),
        (&b"BREENFS"[..], false),
        (&b""[..], false),
        (&[0; 8][..], false),
    ] {
        assert_eq!(mount_args::has_magic(bytes), expected, "{bytes:?}");
    }
}
