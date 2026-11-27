//! Standalone host regression for the metal installer. Compile this entry point
//! directly with rustc.
#[allow(dead_code)]
#[path = "../userspace/programs/src/persistent_install.rs"]
mod installer;

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn image(root: &Path, name: &str) {
    fs::create_dir(root).unwrap();
    for dir in ["bin", "etc", "root"] {
        fs::create_dir(root.join(dir)).unwrap();
    }
    fs::write(root.join("bin/payload"), name).unwrap();
}

fn main() {
    let scratch = std::env::args_os().nth(1).expect("an empty fixture directory");
    let scratch = Path::new(&scratch);
    let old = scratch.join("old-image");
    let new = scratch.join("new-image");
    let system = scratch.join("system");
    image(&old, "old image");
    image(&new, "new image");
    fs::create_dir(&system).unwrap();
    fs::write(old.join("bin/retired"), b"old image only").unwrap();
    let result = installer::install_tree_from(&old, &system, "old", None, &mut || Ok(()));
    assert!(result.is_ok(), "{:?}", result.err());
    let live = system.join(".breenix-generations/old");
    fs::write(live.join("bin/payload"), b"delivered on the box").unwrap();
    fs::set_permissions(live.join("bin/payload"), fs::Permissions::from_mode(0o4755)).unwrap();
    fs::write(live.join("bin/local"), b"created on the box").unwrap();
    fs::write(live.join("bin/adopted"), b"identical to new image").unwrap();
    fs::write(new.join("bin/adopted"), b"identical to new image").unwrap();

    let error = installer::install_tree_from(&new, &system, "new", Some("old"), &mut || {
        panic!("the guard must run before the installer changes the system")
    }).err().expect("upgrading with local files must refuse");
    assert!(error.contains("/bin/local"), "{error}");
    assert!(error.contains("/bin/payload"), "{error}");
    assert!(!error.contains("/bin/retired"), "{error}");
    assert!(!error.contains("/bin/adopted"), "{error}");
    assert!(!system.join(".breenix-generations/new.new").exists());

    let result = installer::install_tree_with_local(&new, &system, "new", Some("old"), true, None, &mut || Ok(()));
    assert!(result.is_ok(), "{:?}", result.err());
    let next = system.join(".breenix-generations/new");
    assert_eq!(fs::read(next.join("bin/payload")).unwrap(), b"new image");
    assert_eq!(fs::read(next.join("bin/local")).unwrap(), b"created on the box");
    let archived = next.join(".breenix-carried/old/bin/payload");
    assert_eq!(fs::read(&archived).unwrap(), b"delivered on the box");
    assert_eq!(fs::metadata(archived).unwrap().permissions().mode() & 0o7777, 0o755);
    assert!(!next.join("bin/retired").exists());
    assert_eq!(fs::read(live.join("bin/payload")).unwrap(), b"delivered on the box");
}
