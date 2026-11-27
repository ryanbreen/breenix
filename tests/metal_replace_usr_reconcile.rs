//! Link to a host libbreenix built with the std feature. Compile without
//! --test so the unrelated legacy installer test module is not included.
#[allow(dead_code)]
#[path = "../userspace/programs/src/persistent_install.rs"]
mod installer;

use libbreenix::etc_reconcile::reconcile_selected;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn main() {
    let path = std::env::temp_dir().join(format!("replace-usr-reconcile-{}", std::process::id()));
    fs::create_dir(&path).unwrap();
    let scratch = Scratch(path);
    let source = scratch.0.join("image");
    let target = scratch.0.join("installed");
    let factory = "usr/share/factory/etc";
    let services = "breenix/services";
    let generation = "usr/share/factory/generation";
    let record = target.join("etc/.breenix-factory");
    fs::create_dir_all(target.join("etc")).unwrap();
    fs::set_permissions(target.join("etc"), fs::Permissions::from_mode(0o755)).unwrap();
    for root in [&target, &source] {
        fs::create_dir_all(root.join(factory).join("breenix")).unwrap();
    }
    let original = b"original service table\n";
    fs::write(target.join(factory).join(services), original).unwrap();
    fs::write(target.join(generation), b"installed\n").unwrap();
    fs::write(target.join("usr/share/retired"), b"old data").unwrap();
    fs::write(source.join(factory).join(services), b"incoming service table\n").unwrap();
    fs::write(source.join("usr/share/replacement"), b"new data").unwrap();
    reconcile_selected(&target).unwrap();
    let original_record = fs::read(&record).unwrap();
    assert_eq!(fs::read(target.join("etc").join(services)).unwrap(), original);

    installer::replace_usr(&source, &target).unwrap();
    // Legacy init creates the layout.tsv directory before reconcile_selected.
    fs::create_dir_all(target.join(factory)).unwrap();
    reconcile_selected(&target).unwrap();

    assert_eq!(fs::read(target.join("etc").join(services)).ok().as_deref(), Some(original.as_slice()));
    assert_eq!(fs::read(&record).unwrap(), original_record);
    assert_eq!(fs::read(target.join(generation)).unwrap(), b"installed\n");
    assert_eq!(fs::read(target.join(factory).join(services)).unwrap(), original);
    assert!(!target.join("usr/share/retired").exists());
    assert_eq!(fs::read(target.join("usr/share/replacement")).unwrap(), b"new data");
}
