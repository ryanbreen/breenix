#[path = "../kernel/src/fs/ufs/args.rs"]
mod args;

#[test]
fn mount_data_is_one_native_fd_and_flags_are_refused() {
    assert_eq!(args::parse(0, b"ufs", 0, &7u32.to_ne_bytes()), Ok(7));
    assert_eq!(args::parse(0, b"ufs", 0, &i32::MAX.to_ne_bytes()), Ok(i32::MAX));
    for data in [&b""[..], &b"fd=7"[..3], &b"fd=7\0"[..]] {
        assert_eq!(args::parse(0, b"ufs", 0, data), Err(22));
    }
    assert_eq!(args::parse(0, b"ufs", 0, &u32::MAX.to_ne_bytes()), Err(9));
    assert_eq!(args::parse(0, b"ext2", 0, &7u32.to_ne_bytes()), Err(22));
    assert_eq!(args::parse(0, b"ufs", 1, &7u32.to_ne_bytes()), Err(22));
}

#[test]
fn mount_and_umount_refuse_nonroot_before_invalid_arguments() {
    assert_eq!(args::require_root(0), Ok(()));
    for uid in [1, 1000, u32::MAX] {
        assert_eq!(args::require_root(uid), Err(1));
        assert_eq!(args::parse(uid, b"ufs", 0, &7u32.to_ne_bytes()), Err(1));
        assert_eq!(args::parse(uid, b"bad", u64::MAX, b""), Err(1));
    }
}
