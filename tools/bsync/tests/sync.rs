use bsync::remote::{listing, parse_listing, Destination};
use bsync::tree::{plan, scan, Action, Entry, Excludes};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        fs::create_dir_all(&base).unwrap();
        let root = base.join(format!(
            "bsync-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn file(&self, path: &str, contents: &str, mode: u32) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn steps(actions: &[Action]) -> Vec<(&'static str, &str)> {
    actions
        .iter()
        .map(|action| (action.label(), action.path()))
        .collect()
}

#[test]
fn trailing_slash_excludes_build_at_any_depth_from_deletion() {
    let local = Fixture::new();
    let remote = Fixture::new();
    remote.file("nested/build/keep", "protected", 0o644);
    remote.file("nested/build-output", "remote only", 0o644);
    let excludes = Excludes::new(&["build/".into()]).unwrap();
    let src = scan(&local.0, &excludes).unwrap();
    let dst = scan(&remote.0, &Excludes::default()).unwrap();
    assert_eq!(
        steps(&plan(&src, &dst, &excludes, true).unwrap()),
        [("DELETE", "nested/build-output")]
    );
}

#[test]
fn plans_content_metadata_and_deletions_from_fixture_trees() {
    let local = Fixture::new();
    fs::set_permissions(&local.0, fs::Permissions::from_mode(0o755)).unwrap();
    local.file("changed", "new", 0o644);
    local.file("mode only", "same", 0o755);
    local.file("nested/deep/new", "new", 0o640);
    local.file("identical", "same", 0o644);
    local.file("ignore.tmp", "ignored", 0o644);

    let remote = Fixture::new();
    fs::set_permissions(&remote.0, fs::Permissions::from_mode(0o700)).unwrap();
    remote.file("changed", "old", 0o644);
    remote.file("mode only", "same", 0o644);
    remote.file("identical", "same", 0o644);
    remote.file("deleted/old", "remote only", 0o644);
    remote.file("keep/ignore.tmp", "protected", 0o644);
    remote.file("keep/stale", "remote only", 0o644);
    remote.file("cache/subdirectory/keep", "protected", 0o644);

    let excludes = Excludes::new(&["*.tmp".into(), "cache/**".into()]).unwrap();
    let src = scan(&local.0, &excludes).unwrap();
    let dst = scan(&remote.0, &Excludes::default()).unwrap();
    let with_deletes = plan(&src, &dst, &excludes, true).unwrap();
    let without_deletes = plan(&src, &dst, &excludes, false).unwrap();

    assert_eq!(
        steps(&with_deletes),
        [
            ("MKDIR", "nested"),
            ("MKDIR", "nested/deep"),
            ("UPLOAD", "changed"),
            ("CHMOD", "mode only"),
            ("UPLOAD", "nested/deep/new"),
            ("DELETE", "deleted/old"),
            ("DELETE", "keep/stale"),
            ("RMDIR", "deleted"),
            ("CHMOD", "nested/deep"),
            ("CHMOD", "nested"),
            ("CHMOD", ""),
        ]
    );
    assert_eq!(
        steps(&without_deletes),
        [
            ("MKDIR", "nested"),
            ("MKDIR", "nested/deep"),
            ("UPLOAD", "changed"),
            ("CHMOD", "mode only"),
            ("UPLOAD", "nested/deep/new"),
            ("CHMOD", "nested/deep"),
            ("CHMOD", "nested"),
            ("CHMOD", ""),
        ]
    );
    let Entry::File { digest, .. } = &src["nested/deep/new"] else {
        panic!("expected a regular file");
    };
    assert_eq!(
        digest,
        "11507a0e2f5e69d5dfa40a62a1bd7b6ee57e6bcd85c67c9b8431b36fff21c437"
    );
}

#[test]
fn refuses_dot_dot_and_symlinks_at_every_destination_level() {
    for path in ["/srv/../tmp/sync", "dir/../sync", "/srv/./sync"] {
        assert!(Destination::parse(&format!("user@host:{path}")).is_err());
    }
    let fixture = Fixture::new();
    let root = fixture.0.canonicalize().unwrap();
    fs::create_dir(root.join("outside")).unwrap();
    fs::create_dir(root.join("dest")).unwrap();
    symlink(root.join("outside"), root.join("dest/link")).unwrap();
    symlink(root.join("outside"), root.join("alias")).unwrap();

    for relative in ["alias/new", "alias", "dest/link", "dest"] {
        let path = root.join(relative);
        let script = listing(path.to_str().unwrap());
        let output = Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        if relative == "dest" {
            assert!(output.status.success(), "{output:?}");
            let remote = parse_listing(&output.stdout).unwrap();
            let local = Fixture::new();
            local.file("link/new", "local", 0o644);
            let src = scan(&local.0, &Excludes::default()).unwrap();
            assert!(plan(&src, &remote.entries, &Excludes::default(), false).is_err());
            assert!(plan(&src, &remote.entries, &Excludes::default(), true).is_err());
        } else {
            assert!(!output.status.success(), "followed {}", path.display());
        }
    }

    symlink(root.join("outside"), root.join("broken-local")).unwrap();
    assert!(scan(&root.join("broken-local"), &Excludes::default()).is_err());
    assert!(!root.join("outside/new").exists());
}

#[test]
fn parses_framed_listing_and_rejects_malformed_records() {
    const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let root = "bsync1\0/srv/tree\0d\0\0drwxr-x--- 2 0 0 4096 Jan 01 00:00 .\0\0";
    let dir = "d\0nested\0drwxrwxr-x 2 0 0 4096 Jan 01 00:00 ./nested\0\0";
    let file = |path: &str, digest: &str| {
        format!("f\0{path}\0-rwxr-x--- 1 12 34 3 Jan 01 00:00 ./nested/space 'file\0{digest}  -\0")
    };
    let good = format!("{root}{dir}{}end\0", file("nested/space 'file", DIGEST));
    let listing = parse_listing(good.as_bytes()).unwrap();
    assert_eq!(listing.root, "/srv/tree");
    assert_eq!(listing.entries["nested"], Entry::Dir(0o775));
    assert_eq!(
        listing.entries["nested/space 'file"],
        Entry::File {
            mode: 0o750,
            size: 3,
            digest: DIGEST.into(),
        }
    );
    assert!(parse_listing(b"bsync1\0/srv/absent\0end\0")
        .unwrap()
        .entries
        .is_empty());
    for bad in [
        format!("{root}{dir}{}", file("nested/a", DIGEST)),
        format!("{root}{dir}{}end\0", file("nested/../escape", DIGEST)),
        format!("{root}{dir}{}end\0", file("/absolute", DIGEST)),
        format!("{root}{dir}{}end\0", file("nested/a", "bad digest")),
        format!("{root}{}end\0", file("missing/a", DIGEST)),
        format!("{root}{dir}{dir}end\0"),
        format!("{root}f\0short\0end\0"),
        format!("{good}extra\0"),
    ] {
        assert!(parse_listing(bad.as_bytes()).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn type_replacement_needs_delete_and_preserves_excluded_children() {
    let local = Fixture::new();
    local.file("replace", "file", 0o644);
    let remote = Fixture::new();
    remote.file("replace/old", "former child", 0o644);
    let no_filter = Excludes::default();
    let src = scan(Path::new(&local.0), &no_filter).unwrap();
    let dst = scan(Path::new(&remote.0), &no_filter).unwrap();
    assert!(plan(&src, &dst, &no_filter, false).is_err());
    assert_eq!(
        steps(&plan(&src, &dst, &no_filter, true).unwrap()),
        [
            ("DELETE", "replace/old"),
            ("RMDIR", "replace"),
            ("UPLOAD", "replace")
        ]
    );
    let protected = Excludes::new(&["replace/old".into()]).unwrap();
    assert!(plan(&src, &dst, &protected, true).is_err());
}
