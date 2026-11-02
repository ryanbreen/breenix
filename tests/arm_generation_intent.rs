//! ARM root generation selection from boot intent.
#[path = "../kernel/src/boot/intent.rs"]
pub mod intent;

#[test]
fn only_persistent_format_two_selects_a_root_generation() {
    fn select(bytes: Option<&[u8]>) -> Result<Option<String>, &'static str> {
        let parsed = bytes.and_then(intent::parse);
        intent::root_userland(parsed.as_ref()).map(|id| id.map(str::to_owned))
    }

    assert_eq!(select(Some(b"BREENIX-BOOT 2\ntarget persistent\nuserland install-42\n")), Ok(Some("install-42".to_owned())));
    assert_eq!(select(Some(b"BREENIX-BOOT 2\ntarget persistent-install\nuserland install-42\n")), Ok(None));
    assert_eq!(select(Some(b"BREENIX-BOOT 2\ntarget persistent\n")), Ok(None));
    assert_eq!(select(Some(b"BREENIX-BOOT 1\npersistent\n")), Ok(None));
    assert_eq!(select(None), Ok(None));
    assert_eq!(select(Some(b"BREENIX-BOOT 2\ntarget persistent\nuserland bad/id\n")), Err("invalid userland generation"));
}
