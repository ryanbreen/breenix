extern crate alloc;

#[path = "../kernel/src/fs/ufs/mounts_line.rs"]
mod mounts_line;

#[test]
fn formats_plain_and_escaped_mounts() {
    let plain = mounts_line::format_mount_line(b"//host/share", b"/mnt/share");
    assert_eq!(AsRef::<[u8]>::as_ref(&plain), b"//host/share /mnt/share ufs rw 0 0\n");
    let escaped = mounts_line::format_mount_line(b"a b\tc\nd\\e", b"/a b\tc\nd\\e");
    assert_eq!(
        AsRef::<[u8]>::as_ref(&escaped),
        b"a\\040b\\011c\\012d\\134e /a\\040b\\011c\\012d\\134e ufs rw 0 0\n"
    );
}

#[test]
fn preserves_path_bytes_outside_the_four_escapes() {
    let line = mounts_line::format_mount_line(b"//host/sh\xff are", b"/mnt/\xc3\xa9\xfe");
    assert_eq!(
        AsRef::<[u8]>::as_ref(&line),
        b"//host/sh\xff\\040are /mnt/\xc3\xa9\xfe ufs rw 0 0\n"
    );
}
