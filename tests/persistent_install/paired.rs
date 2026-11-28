#![allow(dead_code)] // Pulls in the standalone installer, including its guest-only entry points.
#[path = "../../userspace/programs/src/persistent_install.rs"]
mod installer;

use std::{
    fs,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "paired-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for dir in [
            "bin",
            "sbin",
            "lib",
            "usr/lib/breenix",
            "home",
            ".breenix-incoming/objects",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        Self(root)
    }

    fn write(&self, path: &str, bytes: &[u8], mode: u32) {
        fs::write(self.0.join(path), bytes).unwrap();
        fs::set_permissions(self.0.join(path), fs::Permissions::from_mode(mode)).unwrap();
    }

    fn inventory(&self, rows: &[(&str, &str, u32, &str, &[u8])]) -> Vec<u8> {
        let rows: Vec<(&[u8], &str, u32, &[u8], &[u8])> = rows
            .iter()
            .map(|&(path, kind, mode, group, value)| {
                (path.as_bytes(), kind, mode, group.as_bytes(), value)
            })
            .collect();
        self.inventory_bytes(&rows)
    }

    /// Like `inventory`, with each path and group as raw bytes.
    fn inventory_bytes(&self, rows: &[(&[u8], &str, u32, &[u8], &[u8])]) -> Vec<u8> {
        let meta = fs::metadata(&self.0).unwrap();
        let mut rows = rows.to_vec();
        rows.sort_by_key(|row| row.0);
        rows.iter()
            .map(|(path, kind, mode, group, value)| {
                let (size, digest) = if *kind == "dir" {
                    (0, "-".into())
                } else {
                    let digest = installer::paired::digest(value);
                    self.write(&format!(".breenix-incoming/objects/{digest}"), value, 0o600);
                    (value.len(), digest)
                };
                let hex = |path: &[u8]| {
                    path.iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                };
                let group = if *group == b"-" {
                    "-".into()
                } else {
                    hex(group)
                };
                format!(
                    "{}\t{kind}\t{mode:04o}\t{}\t{}\t{group}\t{size}\t123.000000000\t{digest}\n",
                    hex(path),
                    meta.uid(),
                    meta.gid()
                )
            })
            .collect::<String>()
            .into_bytes()
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
        ("/bin/su", "reg", 0o4755, "-", b"new-su"),
        ("/usr/link", "symlink", 0o777, "-", b"missing/target"),
        ("/usr/restore", "reg", 0o644, "-", b"required"),
    ];
    for cut in 0..payload.len() {
        let f = Fixture::new();
        f.write("bin/dropped", b"retired", 0o644);
        f.write("usr/restore", b"old", 0o644);
        f.write("bin/su", b"old", 0o4755);
        let pre = installer::paired::record_fixture(&f.0).unwrap();
        f.write(".breenix-image", pre.as_ref(), 0o600);
        f.write("bin/su", b"pet", 0o4755);
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
        assert!(
            installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || {
                seen += 1;
                if seen == cut + 1 {
                    Err("cut".into())
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let receipt =
            installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || Ok(())).unwrap();
        assert!(receipt.contains("/usr/restore"));
        assert_eq!(
            installer::paired::verify_fixture(&f.0, &inventory).unwrap(),
            installer::paired::digest(&inventory)
        );
        assert_eq!(f.read("bin/su"), b"new-su");
        assert_eq!(f.meta("bin/su").mode() & 0o7777, 0o4755);
        let carried = format!(".breenix-carried/{OLD}/bin/su");
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
        assert_eq!(
            fs::read_link(f.0.join("usr/link")).unwrap(),
            Path::new("missing/target")
        );
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
    symlink(
        f.0.join("home/keep"),
        f.0.join("usr/lib/breenix/rust-current"),
    )
    .unwrap();
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
    assert_eq!(
        f.read(&format!(".breenix-carried/{OLD}/lib/runtime")),
        b"pet"
    );
}

#[test]
fn interrupted_toolchain_conversion_preserves_previous_selection() {
    let booted = Fixture::new();
    let previous = Fixture::new();
    let shared = Fixture::new();
    fs::create_dir_all(shared.0.join("breenix/rust")).unwrap();
    shared.write("breenix/rust/CURRENT", b"old-version\n", 0o644);
    let current = shared.0.join("breenix/rust/CURRENT");
    let select = "usr/lib/breenix/rust-current";
    let path = previous.0.as_path();
    let convert = |writable, checkpoint: &mut dyn FnMut() -> Result<(), String>| {
        installer::paired::convert_fixture(&booted.0, &shared.0, &[(path, writable)], checkpoint)
    };
    assert!(convert(false, &mut || Ok(()))
        .unwrap_err()
        .contains("PREVIOUS"));
    assert_eq!(fs::read(&current).unwrap(), b"old-version\n");
    assert!(!previous.0.join(select).exists());
    assert_eq!(
        convert(true, &mut || Err("power cut".into())).unwrap_err(),
        "power cut"
    );
    assert_eq!(previous.read(select), b"old-version\n");
    assert_eq!(fs::read(&current).unwrap(), b"old-version\n");
    assert!(!booted.0.join(select).exists());
    convert(true, &mut || Ok(())).unwrap();
    assert_eq!(booted.read(select), previous.read(select));
    assert_eq!(
        fs::read_link(&current).unwrap(),
        Path::new("/usr/lib/breenix/rust-current")
    );

    fs::remove_dir_all(shared.0.join("breenix/rust")).unwrap();
    fs::create_dir_all(shared.0.join("home")).unwrap();
    shared.write("home/CURRENT", b"outside", 0o644);
    symlink(shared.0.join("home"), shared.0.join("breenix/rust")).unwrap();
    assert!(convert(true, &mut || Ok(())).is_err());
    assert_eq!(shared.read("home/CURRENT"), b"outside");
}

#[test]
fn runtime_readback_rejects_mismatched_bytes() {
    let root = Fixture::new();
    let opt = Fixture::new();
    let version = opt.0.join("old-version");
    fs::create_dir_all(version.join("bin")).unwrap();
    opt.write("old-version/bin/rustc", b"rustc", 0o755);
    root.write("usr/lib/breenix/rust-current", b"old-version\n", 0o644);
    root.write("lib/libc.so", b"libc", 0o755);
    root.write("lib/ld-breenix.so", b"loader", 0o755);
    let facts = |bytes: &[u8]| {
        serde_json::json!({
            "size": bytes.len(), "mode": "0755", "sha256": installer::paired::digest(bytes)
        })
    };
    let manifest = serde_json::json!({
        "id": "old-version", "files": {"bin/rustc": facts(b"rustc")},
        "runtime": {"libc.so": facts(b"libc"), "ld-breenix.so": facts(b"loader")}
    });
    opt.write(
        "old-version/MANIFEST.json",
        manifest.to_string().as_bytes(),
        0o444,
    );
    installer::paired::verify_runtime_fixture(&root.0, &opt.0).unwrap();
    root.write("lib/libc.so", b"oops", 0o755);
    assert!(installer::paired::verify_runtime_fixture(&root.0, &opt.0)
        .unwrap_err()
        .contains("libc.so"));
}

#[test]
fn publication_keeps_epoch_outside_the_draft_and_refuses_changed_or_stale_boot() {
    let f = Fixture::new();
    f.write("home/keep", b"home", 0o644);
    let inventory = f.inventory(&[("/bin/tool", "reg", 0o755, "-", b"application")]);
    let state = Fixture::new();
    state.write("epoch", b"10\n1-2-3\ngen-old\n", 0o600);
    installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || Ok(())).unwrap();
    assert!(!f.0.join(".breenix-incoming").exists());
    let e1 = state.read("epoch");

    let mut steps = Vec::new();
    let mut publish = |boot, epoch, pending| {
        steps.clear();
        let result = installer::paired::publish_fixture(&e1, boot, "gen-old", epoch, |op| {
            steps.push(op.to_string());
            if op == "pending" && pending {
                Err("pending keys".into())
            } else {
                Ok(())
            }
        });
        (result, steps.clone())
    };
    assert_eq!(
        publish("1-2-3", 10, false),
        (
            Ok(()),
            vec!["detach".into(), "pending".into(), "rename".into()]
        )
    );
    let (result, steps) = publish("1-2-3", 11, false);
    assert!(result.unwrap_err().contains("epoch 10 -> 11"));
    assert_eq!(steps, ["detach", "thaw", "delete"]);
    let (result, steps) = publish("1-2-3", 10, true);
    assert_eq!(result.unwrap_err(), "pending keys");
    assert_eq!(steps, ["detach", "pending", "thaw"]);
    let (result, steps) = publish("4-5-6", 10, false);
    assert!(result.unwrap_err().contains("another boot"));
    assert!(steps.is_empty());
}

#[test]
fn reclaim_restarts_after_each_durable_delete_and_each_draft_publication_state() {
    let booted = format!("gen-{OLD}");
    let previous = "gen-20261127T224742Z-metal".to_string();
    let unpromoted = "gen-20261127T234000Z-metal".to_string();
    let draft = format!("{ID}.new");
    let published = format!("gen-{ID}");
    let retained = vec![previous.clone(), unpromoted.clone()];

    for snapshot in [None, Some(&draft), Some(&published)] {
        let mut initial = std::collections::BTreeMap::from([
            ("default".to_string(), 1),
            (booted.clone(), 2),
            (previous.clone(), 3),
            (unpromoted.clone(), 4),
            ("gen-keep-by-hand".to_string(), 5),
            ("gen-20261127T220000Z-metal".to_string(), 6),
        ]);
        if let Some(name) = snapshot {
            initial.insert(name.clone(), 7);
        }
        let expected = initial.len() - 5;
        for cut in 0..=expected {
            let mut roots = initial.clone();
            let candidates = installer::paired::reclaim_fixture(&roots, &retained, &booted);
            assert_eq!(candidates.len(), expected);
            for name in candidates.into_iter().take(cut) {
                roots.remove(&name).unwrap();
            }
            for name in installer::paired::reclaim_fixture(&roots, &retained, &booted) {
                roots.remove(&name).unwrap();
            }
            assert_eq!(roots.len(), 5);
            for kept in [
                "default",
                &booted,
                &previous,
                &unpromoted,
                "gen-keep-by-hand",
            ] {
                assert!(roots.contains_key(kept));
            }
        }
    }
}

/// The built image carries `rust-std-child_test` and a hard link to it whose final byte is
/// 0xff (scripts/install-layout.sh), for the rust-std suite's non-UTF-8 current_exe case.
/// Apply must record that path in the draft's baseline, and the next apply must read it back.
/// Linux only: APFS refuses a file name that is not UTF-8 (EILSEQ) before the installer sees it.
#[cfg(target_os = "linux")]
#[test]
fn applies_and_rereads_a_non_utf8_payload_path() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    const CHILD: &[u8] = b"/usr/libexec/breenix/rust-std-child_test";
    const LINK: &[u8] = b"/usr/libexec/breenix/rust-std-child_test\xff";
    const NEXT: &str = "20261128T000000Z-metal";
    let rows: &[(&[u8], &str, u32, &[u8], &[u8])] = &[
        (b"/usr/libexec", "dir", 0o755, b"-", b""),
        (b"/usr/libexec/breenix", "dir", 0o755, b"-", b""),
        (CHILD, "reg", 0o755, CHILD, b"child"),
        (LINK, "reg", 0o755, CHILD, b"child"),
    ];
    let f = Fixture::new();
    f.write("home/keep", b"home", 0o644);
    let inventory = f.inventory_bytes(rows);
    installer::paired::apply_fixture(&f.0, ID, OLD, &inventory, &mut || Ok(())).unwrap();
    assert_eq!(
        installer::paired::verify_fixture(&f.0, &inventory).unwrap(),
        installer::paired::digest(&inventory)
    );
    let link = f.0.join(OsStr::from_bytes(&LINK[1..]));
    assert_eq!(fs::read(&link).unwrap(), b"child");
    assert_eq!(fs::metadata(&link).unwrap().ino(), f.meta("usr/libexec/breenix/rust-std-child_test").ino());
    let baseline = f.read(".breenix-image");
    let row = [&b"\t"[..], &LINK[1..], b"\0"].concat();
    assert!(baseline.windows(row.len()).any(|window| window == row));

    // A second generation over the first reads that baseline back.
    fs::create_dir_all(f.0.join(".breenix-incoming/objects")).unwrap();
    let inventory = f.inventory_bytes(rows);
    installer::paired::apply_fixture(&f.0, NEXT, ID, &inventory, &mut || Ok(())).unwrap();
    assert!(!f.0.join(format!(".breenix-carried/{ID}")).exists());
    assert_eq!(fs::read(&link).unwrap(), b"child");
    assert_eq!(f.read("home/keep"), b"home");
}
