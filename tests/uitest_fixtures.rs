#[path = "../libs/libuitest/src/fixtures.rs"]
pub mod fixtures;

#[test]
fn fresh_fixture_tree_and_font_locale_pins_reject_tampering() {
    let root = std::env::temp_dir().join(format!("uitest-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("stale"), b"previous case").unwrap();
    fixtures::prepare(&root).unwrap();
    assert!(!root.join("stale").exists());
    fixtures::verify_at(&root, std::path::Path::new("fonts")).unwrap();
    for relative in ["fixtures/names.txt", "etc/locale.conf", "etc/fonts.conf"] {
        let path = root.join(relative);
        let before = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"tampered").unwrap();
        assert!(fixtures::verify_at(&root, std::path::Path::new("fonts")).is_err());
        std::fs::write(&path, before).unwrap();
    }
    std::fs::write(root.join("fixtures/unpinned"), b"extra").unwrap();
    assert!(fixtures::verify_at(&root, std::path::Path::new("fonts")).is_err());
    std::fs::remove_file(root.join("fixtures/unpinned")).unwrap();
    let fonts = root.join("fonts");
    std::fs::create_dir(&fonts).unwrap();
    for name in ["DejaVuSans.ttf", "DejaVuSansMono.ttf"] {
        std::fs::copy(std::path::Path::new("fonts").join(name), fonts.join(name)).unwrap();
    }
    std::fs::write(fonts.join("DejaVuSans.ttf"), b"wrong font").unwrap();
    assert!(fixtures::verify_at(&root, &fonts).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
