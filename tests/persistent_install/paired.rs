#![allow(dead_code)] // Pulls in the standalone installer, including its guest-only entry points.
#[path = "../../userspace/programs/src/persistent_install.rs"]
mod installer;

use std::{fs, os::unix::fs::{symlink, MetadataExt, PermissionsExt}, path::{Path, PathBuf}};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("paired-{}-{}", std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        for dir in ["bin", "sbin", "lib", "usr/lib/breenix", "home", ".breenix-incoming/objects"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        Self(root)
    }

    fn write(&self, path: &str, bytes: &[u8], mode: u32) {
        fs::write(self.0.join(path), bytes).unwrap();
        fs::set_permissions(self.0.join(path), fs::Permissions::from_mode(mode)).unwrap();
    }

    fn inventory(&self, rows: &[(&str, &str, u32, &str, &[u8])]) -> Vec<u8> {
        let meta = fs::metadata(&self.0).unwrap();
        let mut rows = rows.to_vec();
        rows.sort_by_key(|row| row.0);
        rows.iter().map(|(path, kind, mode, group, value)| {
            let (size, digest) = if *kind == "dir" {
                (0, "-".into())
            } else {
                let digest = installer::paired::digest(value);
                self.write(&format!(".breenix-incoming/objects/{digest}"), value, 0o600);
                (value.len(), digest)
            };
            let hex = |path: &str| path.bytes().map(|byte| format!("{byte:02x}")).collect::<String>();
            let group = if *group == "-" { "-".into() } else { hex(group) };
            format!("{}\t{kind}\t{mode:04o}\t{}\t{}\t{group}\t{size}\t123.000000000\t{digest}\n",
                hex(path), meta.uid(), meta.gid())
        }).collect::<String>().into_bytes()
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.0.join(path)).unwrap()
    }

    fn meta(&self, path: &str) -> fs::Metadata {
        fs::metadata(self.0.join(path)).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

const ID: &str = "20261127T235000Z-metal";
const OLD: &str = "20261127T230000Z-metal";

#[test]
fn applies_inventory_over_clone_and_resumes_at_each_file() {
    let payload: &[(&str, &str, u32, &str, &[u8])] = &[
        ("/bin/busybox", "reg", 0o755, "/bin/busybox", b"busy"),
        ("/bin/echo", "reg", 0o755, "/bin/busybox", b"busy"),
        ("/bin/first", "reg", 0o644, "-", b"same"),
        ("/bin/second", "reg", 0o644, "-", b"same"),
        ("/lib/runtime", "reg", 0o644, "-", b"runtime"),
        ("/sbin/getty", "reg", 0o755, "/bin/busybox", b"busy"),
        ("/sbin/su", "reg", 0o4755, "-", b"new-su"),
        ("/usr/link", "symlink", 0o777, "-", b"missing/target"),
        ("/usr/restore", "reg", 0o644, "-", b"required"),
    ];
    for cut in 0..payload.len() {
        let f = Fixture::new();
        f.write("bin/dropped", b"retired", 0o644);
        f.write("usr/restore", b"old", 0o644);
        f.write("sbin/su", b"old", 0o4755);
        let pre = installer::paired::record_fixture(&f.0).unwrap();
        f.write(".breenix-image", pre.as_bytes(), 0o600);
        f.write("sbin/su", b"pet", 0o4755);
        f.write("usr/pet", b"pet", 0o644);
        f.write("home/keep", b"home", 0o644);
        f.write("bin/busybox", b"busy", 0o755);
        f.write("bin/echo", b"busy", 0o755);
        f.write("sbin/getty", b"busy", 0o755);
        fs::hard_link(f.0.join("bin/busybox"), f.0.join("bin/pet-link")).unwrap();
        fs::remove_file(f.0.join("usr/restore")).unwrap();
        let inventory = f.inventory(payload);
        let have = installer::paired::have_fixture(&f.0, &inventory).unwrap();
        for member in ["/bin/busybox", "/bin/echo", "/sbin/getty"] {
            assert!(!have.iter().any(|entry| entry == member));
        }
        let mut seen = 0;
        assert!(installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || {
            seen += 1;
            if seen == cut + 1 { Err("cut".into()) } else { Ok(()) }
        }).is_err());
        let receipt = installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || Ok(())).unwrap();
        assert!(receipt.contains("/usr/restore"));
        assert_eq!(installer::paired::verify_fixture(&f.0, &inventory).unwrap(),
                   installer::paired::digest(&inventory));
        assert_eq!(f.read("sbin/su"), b"new-su");
        assert_eq!(f.meta("sbin/su").mode() & 0o7777, 0o4755);
        let carried = format!(".breenix-carried/{OLD}/sbin/su");
        assert_eq!(f.read(&carried), b"pet");
        assert_eq!(f.meta(&carried).mode() & 0o6000, 0);
        assert_eq!(f.read("bin/pet-link"), b"busy");
        assert_eq!(f.meta("bin/pet-link").nlink(), 1);
        assert_eq!(f.meta("bin/busybox").nlink(), 3);
        assert_eq!(f.meta("bin/busybox").ino(), f.meta("sbin/getty").ino());
        assert_ne!(f.meta("bin/first").ino(), f.meta("bin/second").ino());
        assert_eq!(f.read("usr/pet"), b"pet");
        assert_eq!(f.read("home/keep"), b"home");
        assert!(!f.0.join("bin/dropped").exists());
        assert!(!f.0.join(".breenix-incoming").exists());
        assert_eq!(fs::read_link(f.0.join("usr/link")).unwrap(), Path::new("missing/target"));
    }
}

#[test]
fn refuses_symlinked_parents_and_selection_leaves() {
    let f = Fixture::new();
    f.write("home/keep", b"intact", 0o644);
    let inventory = f.inventory(&[("/usr/lib/keep", "reg", 0o644, "-", b"bad")]);
    fs::remove_dir_all(f.0.join("usr/lib")).unwrap();
    symlink(f.0.join("home"), f.0.join("usr/lib")).unwrap();
    assert!(installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || Ok(())).is_err());
    assert!(installer::paired::select_fixture(&f.0, "version").is_err());
    fs::remove_file(f.0.join("usr/lib")).unwrap();
    fs::create_dir_all(f.0.join("usr/lib/breenix")).unwrap();
    symlink(f.0.join("home/keep"), f.0.join("usr/lib/breenix/rust-current")).unwrap();
    assert!(installer::paired::select_fixture(&f.0, "version").is_err());
    assert_eq!(f.read("home/keep"), b"intact");
}

#[test]
fn missing_baseline_carries_a_pet_edit_and_unknown_header_refuses() {
    let f = Fixture::new();
    f.write("lib/runtime", b"pet", 0o644);
    let inventory = f.inventory(&[("/lib/runtime", "reg", 0o644, "-", b"fresh")]);
    f.write(".breenix-image", b"BREENIX-IMAGE-99\0", 0o600);
    assert!(installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || Ok(())).is_err());
    assert_eq!(f.read("lib/runtime"), b"pet");
    fs::remove_file(f.0.join(".breenix-image")).unwrap();
    installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || Ok(())).unwrap();
    assert_eq!(f.read(&format!(".breenix-carried/{OLD}/lib/runtime")), b"pet");
}
