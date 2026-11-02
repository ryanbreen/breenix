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

#[path = "../kernel/src/boot/arm_install.rs"]
mod arm_install;

#[test]
fn install_argv_requires_userland_and_preserves_previous() {
    assert_eq!(arm_install::argv(None, None), Err("install intent names no userland generation"));
    assert_eq!(arm_install::argv(Some("install-42"), None), Ok(vec![b"/sbin/persistent-install".as_slice(), b"install-42".as_slice()]));
    assert_eq!(arm_install::argv(Some("install-42"), Some("install-41")), Ok(vec![b"/sbin/persistent-install".as_slice(), b"install-42".as_slice(), b"install-41".as_slice()]));
}
