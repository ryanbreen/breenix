#[path = "../libs/libbreenix/src/layout.rs"]
pub mod layout;

#[test]
fn image_and_generation_walk_refuse_unlisted_setid_inodes() {
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = std::env::temp_dir().join(format!("layout-setid-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    for name in ["opt/nested/program", "etc/unlisted", ".breenix-generations/old/var/unlisted"] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"fixture").unwrap();
        for mode in [0o4755, 0o2755, 0o6755] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(layout::check_setid(&root, false).unwrap_err().contains(name));
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let dir = root.join("srv");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o2775)).unwrap();
    assert!(layout::check_setid(&root, false).unwrap_err().contains("srv"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    symlink("../opt", dir.join("link")).unwrap();
    layout::check_setid(&root, false).unwrap();
    fs::remove_dir_all(root).unwrap();
}
