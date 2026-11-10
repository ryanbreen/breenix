#[path = "../userspace/programs/src/guiapps_suite/blog_support.rs"]
mod support;

#[test]
fn boot_identity_is_created_once_and_preserved() {
    let dir = std::env::temp_dir().join(format!("blogd-boot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("boot-id");
    assert_eq!(support::ensure_boot_id(&path, "first").unwrap(), "first");
    assert_eq!(support::ensure_boot_id(&path, "second").unwrap(), "first");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn each_audit_record_is_one_write() {
    #[derive(Default)] struct Writes(Vec<Vec<u8>>);
    impl std::io::Write for Writes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> { self.0.push(bytes.to_vec()); Ok(bytes.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut out = Writes::default();
    support::write_records(&mut out, "nonce", 0..2).unwrap();
    assert_eq!(out.0, [b"nonce-00\n".to_vec(), b"nonce-01\n".to_vec()]);
}
