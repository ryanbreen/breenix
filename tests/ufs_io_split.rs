#[path = "../kernel/src/fs/ufs/io_split.rs"]
mod io_split;

#[test]
fn chunks_respect_payload_limits_and_stop_on_short_reply() {
    let mut read = io_split::Transfer::new(150_000, 65_536);
    assert_eq!(read.next(), Some(65_536));
    assert!(read.complete(65_536).unwrap());
    assert_eq!(read.next(), Some(65_536));
    assert!(!read.complete(12).unwrap());
    assert_eq!(read.next(), None);
    let mut write = io_split::Transfer::new(150_000, 65_520);
    assert_eq!(write.next(), Some(65_520));
    assert!(write.complete(65_520).unwrap());
    assert_eq!(write.next(), Some(65_520));
    assert!(write.complete(65_521).is_err());
}

#[test]
fn dirent_records_are_aligned_and_keep_the_resume_cookie() {
    let mut out = [0xa5; 64];
    let n = io_split::pack_dirent(&mut out, 42, 73, 8, b"name").unwrap();
    assert_eq!(n, 24);
    assert_eq!(u64::from_ne_bytes(out[0..8].try_into().unwrap()), 42);
    assert_eq!(u64::from_ne_bytes(out[8..16].try_into().unwrap()), 73);
    assert_eq!(u16::from_ne_bytes(out[16..18].try_into().unwrap()), 24);
    assert_eq!(out[18], 8);
    assert_eq!(&out[19..24], b"name\0");
    assert!(io_split::pack_dirent(&mut out[..23], 42, 73, 8, b"name").is_none());
}
