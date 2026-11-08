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
    let saved = std::env::var_os("BREENIX_CONFIG_ROOT");
    std::env::remove_var("BREENIX_CONFIG_ROOT");
    assert_eq!(settings::Paths::environment().hotkeys, Path::new("/etc/hotkeys.conf"));
    std::env::set_var("BREENIX_CONFIG_ROOT", "/tmp/uitest/73/");
    assert_eq!(settings::Paths::environment().bwm, Path::new("/tmp/uitest/73/etc/bwm.conf"));
    match saved { Some(value) => std::env::set_var("BREENIX_CONFIG_ROOT", value), None => std::env::remove_var("BREENIX_CONFIG_ROOT") }
    let root = std::env::temp_dir().join(format!("bwm-config-{}", std::process::id()));
    fs::create_dir_all(root.join("etc")).unwrap();
    let paths = settings::Paths::from_root(Some(&root));
    assert_eq!(settings::Settings::load(&paths.bwm).unwrap(), settings::Settings::default());
    fs::write(&paths.bwm, b"font.size=12\nclock.visible=false\n").unwrap();
    let loaded = settings::Settings::load(&paths.bwm).unwrap();
    assert_eq!(loaded.font_size, 12.0);
    assert!(!loaded.clock_visible);
    fs::write(&paths.bwm, b"font.size=NaN\n").unwrap();
    assert!(settings::Settings::load(&paths.bwm).is_err());
    fs::remove_dir_all(root).unwrap();
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
    for path in ["/etc/fonts.conf", "/etc/hotkeys.conf", "/etc/bwm.conf", "/usr/share/applications"] { assert!(error.contains(path), "{error}"); }
    assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(), b"fonts");
    assert!(!root.join("etc/bwm.conf").exists());
    assert_eq!(fs::read(root.join("usr/share/applications/link")).unwrap(), b"catalog");
    assert_eq!(fs::read(root.join("var/log/extra")).unwrap(), b"log");
    assert_ne!(fs::metadata(root.join("etc/hotkeys.conf")).unwrap().permissions().mode() & 0o777, 0o600);
    let baseline = config::Snapshot::capture_at(&root).unwrap();
    fs::remove_file(root.join("etc/fonts.conf")).unwrap();
    fs::create_dir(root.join("etc/fonts.conf")).unwrap();
    assert!(baseline.verify_restore().unwrap_err().contains("/etc/fonts.conf"));
    assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(), b"fonts");
    // Restoration unlinks the planted symlink without overwriting its target.
    fs::write(root.join("outside"), b"unrelated").unwrap();
    let baseline = config::Snapshot::capture_at(&root).unwrap();
    fs::remove_file(root.join("etc/fonts.conf")).unwrap();
    symlink(root.join("outside"), root.join("etc/fonts.conf")).unwrap();
    assert!(baseline.verify_restore().unwrap_err().contains("/etc/fonts.conf"));
    assert_eq!(fs::read(root.join("outside")).unwrap(), b"unrelated");
    assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(), b"fonts");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_service_log_appends_survive_custody_with_the_same_open_inode() {
    use std::{io::Write, os::unix::fs::MetadataExt};
    assert_eq!(config::GLOBAL_PATHS.as_slice(), ["/etc/fonts.conf", "/etc/hotkeys.conf", "/etc/bwm.conf", "/usr/share/applications"]);
    let root = std::env::temp_dir().join(format!("service-log-custody-{}", std::process::id()));
    fs::create_dir_all(root.join("var/log")).unwrap();
    let path = root.join("var/log/service.log");
    fs::write(&path, b"before\n").unwrap();
    let mut service = fs::OpenOptions::new().append(true).open(&path).unwrap();
    let before = service.metadata().unwrap();
    let snapshot = config::Snapshot::capture_at(&root).unwrap();
    service.write_all(b"during-case\n").unwrap();
    snapshot.verify_restore().unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(service.metadata().unwrap().nlink(), 1);
    service.write_all(b"after-case\n").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"before\nduring-case\nafter-case\n");
    fs::remove_dir_all(root).unwrap();
}
