extern crate alloc;

#[path = "../kernel/src/fs/ufs/mounts_line.rs"]
mod mounts_line;

#[test]
fn formats_plain_and_escaped_mounts() {
    assert_eq!(mounts_line::format_mount_line(b"//host/share", b"/mnt/share"), "//host/share /mnt/share ufs rw 0 0\n");
    assert_eq!(mounts_line::format_mount_line(b"a b\tc\nd\\e", b"/a b\tc\nd\\e"), "a\\040b\\011c\\012d\\134e /a\\040b\\011c\\012d\\134e ufs rw 0 0\n");
}
