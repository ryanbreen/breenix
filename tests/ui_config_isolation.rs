#[path = "../userspace/programs/src/bwm/settings.rs"]
pub mod settings;
#[path = "../libs/libbreenix/src/suite_config.rs"]
pub mod config;

use std::{fs, path::Path, os::unix::fs::{symlink, PermissionsExt}};

#[test]
fn paths_and_settings_follow_the_root_without_falling_back_to_global_files() {
    for root in [None, Some(Path::new("/tmp/uitest/73/"))] {
        let paths = settings::Paths::from_root(root);
        let base = root.unwrap_or(Path::new("/"));
        assert_eq!(paths.hotkeys, base.join("etc/hotkeys.conf"));
        assert_eq!(paths.bwm, base.join("etc/bwm.conf"));
    }
    assert_eq!(settings::Settings::default().font_size, 14.0);
    assert!(settings::Settings::default().clock_visible);
    let a = settings::Settings::parse("font.size=12\nclock.visible=false\n").unwrap();
    let b = settings::Settings::parse("clock.visible=false\nfont.size=12\n").unwrap();
    assert_eq!(a, b);
    assert_eq!(a.font_size, 12.0);
    assert!(!a.clock_visible);
    for bad in ["font.size=NaN\n", "font.size=0\n", "clock.visible=yes\n", "clock.visible=true\nclock.visible=false\n", "unknown=1\n", "font.size=12"] {
        assert!(settings::Settings::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn planted_content_membership_type_and_mode_changes_are_restored_and_named() {
    let root = std::env::temp_dir().join(format!("config-isolation-{}", std::process::id()));
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::create_dir_all(root.join("usr/share/applications/nested")).unwrap();
    fs::create_dir_all(root.join("var/log")).unwrap();
    fs::write(root.join("etc/fonts.conf"), b"fonts").unwrap();
    fs::write(root.join("etc/hotkeys.conf"), b"keys").unwrap();
    fs::write(root.join("usr/share/applications/nested/app"), b"catalog").unwrap();
    symlink("nested/app", root.join("usr/share/applications/link")).unwrap();
    let baseline = config::Snapshot::capture_at(&root).unwrap();
    // Enumeration order cannot affect the digest.
    config::Snapshot::capture_at(&root).unwrap().verify_restore().unwrap();
    fs::write(root.join("etc/fonts.conf"), b"other").unwrap();
    fs::set_permissions(root.join("etc/hotkeys.conf"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(root.join("etc/bwm.conf"), b"created").unwrap();
    fs::remove_dir_all(root.join("usr/share/applications/nested")).unwrap();
    fs::write(root.join("var/log/extra"), b"log").unwrap();
    let error = baseline.verify_restore().unwrap_err();
    for path in ["/etc/fonts.conf", "/etc/hotkeys.conf", "/etc/bwm.conf", "/usr/share/applications", "/var/log"] { assert!(error.contains(path), "{error}"); }
    assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(), b"fonts");
    assert!(!root.join("etc/bwm.conf").exists());
    assert_eq!(fs::read(root.join("usr/share/applications/link")).unwrap(), b"catalog");
    assert!(fs::read_dir(root.join("var/log")).unwrap().next().is_none());
    assert_ne!(fs::metadata(root.join("etc/hotkeys.conf")).unwrap().permissions().mode() & 0o777, 0o600);
    let baseline = config::Snapshot::capture_at(&root).unwrap();
    fs::remove_file(root.join("etc/fonts.conf")).unwrap();
    fs::create_dir(root.join("etc/fonts.conf")).unwrap();
    assert!(baseline.verify_restore().unwrap_err().contains("/etc/fonts.conf"));
    assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(), b"fonts");
    fs::remove_dir_all(root).unwrap();
}
