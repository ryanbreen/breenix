//! Compile with rustc and pass an empty temporary directory.
#[allow(dead_code)]
#[path = "../userspace/programs/src/persistent_install.rs"]
mod installer;

use std::{fs, path::Path};

fn main() {
    let scratch = std::env::args_os().nth(1).unwrap();
    let source = Path::new(&scratch).join("image");
    let target = Path::new(&scratch).join("selected");
    fs::create_dir_all(source.join("usr/bin")).unwrap();
    fs::create_dir_all(target.join("usr/bin")).unwrap();
    fs::write(source.join("usr/bin/new"), b"replacement").unwrap();
    fs::hard_link(source.join("usr/bin/new"), source.join("usr/bin/alias")).unwrap();
    std::os::unix::fs::symlink("new", source.join("usr/bin/link")).unwrap();
    fs::write(target.join("usr/bin/old"), b"retired").unwrap();
    for dir in ["etc", "var", "home", "root", "opt", "srv", "bin", "sbin"] {
        fs::create_dir_all(target.join(dir)).unwrap();
        fs::create_dir_all(source.join(dir)).unwrap();
        fs::write(target.join(dir).join("local"), dir).unwrap();
        fs::write(source.join(dir).join("image"), b"leave behind").unwrap();
    }
    fs::write(target.join(".complete"), b"original identity").unwrap();
    installer::replace_usr(&source, &target).unwrap();
    assert_eq!(fs::read(target.join("usr/bin/new")).unwrap(), b"replacement");
    use std::os::unix::fs::MetadataExt;
    assert_eq!(fs::metadata(target.join("usr/bin/new")).unwrap().ino(),
               fs::metadata(target.join("usr/bin/alias")).unwrap().ino());
    assert_eq!(fs::read_link(target.join("usr/bin/link")).unwrap(), Path::new("new"));
    assert_eq!(fs::read_dir(target.join("usr/bin")).unwrap().count(), 3);
    assert_eq!(fs::read_dir(target.join("usr")).unwrap().count(), 1);
    for dir in ["etc", "var", "home", "root", "opt", "srv", "bin", "sbin"] {
        assert_eq!(fs::read(target.join(dir).join("local")).unwrap(), dir.as_bytes());
        assert_eq!(fs::read_dir(target.join(dir)).unwrap().count(), 1);
    }
    assert_eq!(fs::read(target.join(".complete")).unwrap(), b"original identity");
}
