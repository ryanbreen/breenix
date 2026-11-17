use crate::breenfs::mount_args::parse_root;
use crate::syscall::errno::EINVAL;

#[test]
fn snapshot_root_mount_data() {
    assert_eq!(parse_root(None), Ok(None));
    assert_eq!(parse_root(Some(b"")), Ok(None));
    assert_eq!(parse_root(Some(b"root=alpha")), Ok(Some(&b"alpha"[..])));
    let longest = [b'a'; 255];
    let mut data = b"root=".to_vec();
    data.extend_from_slice(&longest);
    assert_eq!(parse_root(Some(&data)), Ok(Some(&longest[..])));
    data.push(b'a');
    assert_eq!(parse_root(Some(&data)), Err(EINVAL as u64));
    for bad in [&b"root="[..], b"root=a/b", b"root=a,b", b"root=a\0b", b"other=x"] {
        assert_eq!(parse_root(Some(bad)), Err(EINVAL as u64));
    }
}
