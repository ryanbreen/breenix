//! Reconcile generation defaults with shared /etc, using content hashes rather
//! than timestamps. Written from the generation protocol and POSIX rename rules.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use crate::crypto::sha256::sha256;

const RECORD: &str = ".breenix-factory";
type Result<T> = std::result::Result<T, String>;

fn io<T>(result: std::io::Result<T>, path: &Path) -> Result<T> {
    result.map_err(|e| format!("{}: {}", path.display(), e))
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {}", path.display(), e)),
    }
}
fn digest(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|b| format!("{:02x}", b)).collect()
}
fn sync(path: &Path) -> Result<()> { io(File::open(path).and_then(|f| f.sync_all()), path) }
fn regular(path: &Path) -> Result<Vec<u8>> {
    let meta = io(fs::symlink_metadata(path), path)?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return Err(format!("{}: expected a bounded regular factory file", path.display()));
    }
    io(fs::read(path), path)
}
// Exact v1 image defaults establish provenance before the first v2 record.
// Unknown or locally edited bytes still receive a .factory sibling.
fn legacy_default(name: &str) -> Option<&'static [u8]> {
    match name {
        "bshrc" => Some(include_bytes!("legacy_etc/bshrc")),
        "hotkeys.conf" => Some(include_bytes!("legacy_etc/hotkeys.conf")),
        _ => None,
    }
}
fn machine_owned(name: &str) -> bool {
    matches!(name, "passwd" | "group" | "shadow" | "sudo.policy" | "login.defs") || name.starts_with("ssh/")
}
fn walk(path: &Path, prefix: &str, files: &mut BTreeMap<String, (Vec<u8>, u32)>) -> Result<()> {
    for entry in io(fs::read_dir(path), path)? {
        let entry = io(entry, path)?;
        let name = entry.file_name().into_string().map_err(|_| "non-text factory name")?;
        if name.contains(['\n', '\t', '\r']) || name == RECORD || name.ends_with(".factory") {
            return Err("reserved factory name".into());
        }
        let relative = if prefix.is_empty() { name } else { format!("{}/{}", prefix, name) };
        let child = entry.path();
        let meta = io(fs::symlink_metadata(&child), &child)?;
        if meta.is_dir() { walk(&child, &relative, files)?; }
        else { files.insert(relative, (regular(&child)?, meta.mode() & 0o7777)); }
    }
    Ok(())
}

/// Operation boundaries are exposed to the installer host fault tests. The
/// durable old record remains until the operations succeed. A retry also
/// recognizes new bytes already installed before an interruption.
struct Writer<'a>(&'a mut dyn FnMut() -> Result<()>);
impl Writer<'_> {
    fn change<T>(&mut self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        (self.0)()?;
        let result = operation()?;
        (self.0)()?;
        Ok(result)
    }
    fn parents(&mut self, root: &Path, name: &str) -> Result<PathBuf> {
        let mut path = root.to_path_buf();
        let parts: Vec<_> = Path::new(name).components().collect();
        if parts.iter().any(|p| !matches!(p, Component::Normal(_))) { return Err("unsafe factory path".into()); }
        for part in &parts[..parts.len().saturating_sub(1)] {
            path.push(part.as_os_str());
            if !exists(&path)? {
                self.change(|| io(fs::create_dir(&path), &path))?;
                self.change(|| io(fs::set_permissions(&path, fs::Permissions::from_mode(0o755)), &path))?;
                self.change(|| sync(path.parent().unwrap()))?;
            }
            if !io(fs::symlink_metadata(&path), &path)?.is_dir() { return Err("factory parent is not a directory".into()); }
        }
        Ok(root.join(name))
    }
    fn write(&mut self, path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
        let scratch = path.with_file_name(format!(".{}.reconcile-new", path.file_name().unwrap().to_string_lossy()));
        if exists(&scratch)? { self.change(|| io(fs::remove_file(&scratch), &scratch))?; }
        let mut file = self.change(|| io(OpenOptions::new().write(true).create_new(true).mode(0o600).open(&scratch), &scratch))?;
        self.change(|| io(file.write_all(bytes), &scratch))?;
        self.change(|| io(file.set_permissions(fs::Permissions::from_mode(mode)), &scratch))?;
        self.change(|| io(file.sync_all(), &scratch))?;
        drop(file);
        self.change(|| io(fs::rename(&scratch, path), path))?;
        self.change(|| sync(path.parent().unwrap()))
    }
}

fn parse_record(bytes: &[u8]) -> Result<(String, BTreeMap<String, String>)> {
    let text = std::str::from_utf8(bytes).map_err(|_| "factory record is not text")?;
    let mut lines = text.lines();
    if lines.next() != Some("BREENIX-ETC 1") { return Err("invalid factory record".into()); }
    let generation = lines.next().ok_or("factory record has no generation")?.to_string();
    let mut hashes = BTreeMap::new();
    for line in lines {
        let (hash, name) = line.split_once('\t').ok_or("invalid factory record row")?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || name.is_empty()
            || Path::new(name).components().any(|p| !matches!(p, Component::Normal(_)))
            || hashes.insert(name.into(), hash.into()).is_some() {
            return Err("invalid factory record row".into());
        }
    }
    Ok((generation, hashes))
}

pub fn reconcile(root: &Path, generation: &str, checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<Vec<String>> {
    let factory = root.join("usr/share/factory/etc");
    let etc = root.join("etc");
    let record = etc.join(RECORD);
    let previous = if exists(&record)? { parse_record(&regular(&record)?)? } else { (String::new(), BTreeMap::new()) };
    let mut files = BTreeMap::new();
    walk(&factory, "", &mut files)?;
    let mut writer = Writer(checkpoint);
    let mut conflicts = Vec::new();
    for (name, (bytes, mode)) in &files {
        if previous.0 == generation && !machine_owned(name) { continue; }
        let path = writer.parents(&etc, name)?;
        if !exists(&path)? { writer.write(&path, bytes, *mode)?; continue; }
        if machine_owned(name) {
            // Add required system entries, preserving existing lines byte for byte.
            if matches!(name.as_str(), "passwd" | "group" | "shadow") {
                let existing = regular(&path)?;
                let text = std::str::from_utf8(&existing).map_err(|_| "account database is not text")?;
                let incoming = std::str::from_utf8(bytes).map_err(|_| "factory account database is not text")?;
                let mut merged = text.to_string();
                for line in incoming.lines() {
                    let account = line.split(':').next().unwrap_or("");
                    if !text.lines().any(|old| old.split(':').next() == Some(account)) {
                        if !merged.is_empty() && !merged.ends_with('\n') { merged.push('\n'); }
                        merged.push_str(line); merged.push('\n');
                    }
                }
                if merged.as_bytes() != existing { writer.write(&path, merged.as_bytes(), *mode)?; }
            }
            continue;
        }
        // A symlink or another kind is a local edit. Do not follow it.
        let existing = if io(fs::symlink_metadata(&path), &path)?.is_file() { Some(regular(&path)?) } else { None };
        let hash = existing.as_deref().map(digest);
        if hash.as_deref() == Some(digest(bytes).as_str()) { continue; }
        if hash.as_ref().is_some_and(|h| previous.1.get(name) == Some(h))
            || existing.as_deref().is_some_and(|b| legacy_default(name) == Some(b)) {
            writer.write(&path, bytes, *mode)?;
        } else {
            let sibling = path.with_file_name(format!("{}.factory", path.file_name().unwrap().to_string_lossy()));
            writer.write(&sibling, bytes, *mode)?;
            conflicts.push(name.clone());
        }
    }
    for (name, hash) in &previous.1 {
        if previous.0 == generation { break; }
        if files.contains_key(name) || machine_owned(name) { continue; }
        let path = writer.parents(&etc, name)?;
        if exists(&path)? && io(fs::symlink_metadata(&path), &path)?.is_file() && digest(&regular(&path)?) == *hash {
            writer.change(|| io(fs::remove_file(&path), &path))?;
            writer.change(|| sync(path.parent().unwrap()))?;
        }
    }
    let mut text = format!("BREENIX-ETC 1\n{}\n", generation);
    for (name, (bytes, _)) in &files { text.push_str(&format!("{}\t{}\n", digest(bytes), name)); }
    if previous.0 != generation || !exists(&record)? || regular(&record)? != text.as_bytes() {
        writer.write(&record, text.as_bytes(), 0o600)?;
    }
    Ok(conflicts)
}

/// Run before any service (also when a suite is PID 1). Only the selected
/// generation's factory tree is used; boot-host staging remains outside it.
fn clear_volatile(root: &Path) -> Result<()> {
    for name in ["tmp", "run"] {
        let path = root.join(name);
        if !io(fs::symlink_metadata(&path), &path)?.is_dir() { return Err("volatile path is not a directory".into()); }
        for entry in io(fs::read_dir(&path), &path)? {
            let entry = io(entry, &path)?;
            let path = entry.path();
            if io(fs::symlink_metadata(&path), &path)?.is_dir() { io(fs::remove_dir_all(&path), &path)?; }
            else { io(fs::remove_file(&path), &path)?; }
        }
        sync(&path)?;
    }
    Ok(())
}

pub fn prepare(root: &Path) -> Result<Vec<String>> {
    // Existing generations may predate new standard directories and log modes.
    // Account contents remain governed by the machine-owned reconciliation rule.
    for line in crate::layout::TABLE.lines() {
        let row: Vec<_> = line.split_whitespace().collect();
        if row.len() != 5 || !matches!(row[0], "dir" | "file") { continue; }
        let path = root.join(row[1]);
        let mode = u32::from_str_radix(row[2], 8).map_err(|_| "invalid layout mode")?;
        if row[0] == "dir" {
            if !exists(&path)? { io(fs::create_dir(&path), &path)?; }
            if !io(fs::symlink_metadata(&path), &path)?.is_dir() { return Err("layout directory is not a directory".into()); }
            io(fs::set_permissions(&path, fs::Permissions::from_mode(mode)), &path)?;
            io(std::os::unix::fs::chown(&path, Some(0), Some(0)), &path)?;
        } else if row[1].starts_with("var/") {
            if !exists(&path)? { io(OpenOptions::new().write(true).create_new(true).mode(mode).open(&path), &path)?; }
            if !io(fs::symlink_metadata(&path), &path)?.is_file() { return Err("layout log is not a regular file".into()); }
            io(fs::set_permissions(&path, fs::Permissions::from_mode(mode)), &path)?;
            io(std::os::unix::fs::chown(&path, Some(0), Some(4)), &path)?;
        }
    }
    clear_volatile(root)?;
    reconcile_selected(root)
}

/// Reconcile configuration without clearing live runtime state. This is also
/// the public etc-reconcile command's entry point after an administrator edit.
pub fn reconcile_selected(root: &Path) -> Result<Vec<String>> {
    if !exists(&root.join("usr/share/factory/etc"))? { return Ok(Vec::new()); }
    let identity = root.join("usr/share/factory/generation");
    // The generation namespace hides .complete; the installer publishes the
    // selected identity beside its factory tree instead.
    let generation = if exists(&identity)? { String::from_utf8(regular(&identity)?).map_err(|_| "invalid generation text")? }
        else {
            let mut files = BTreeMap::new();
            walk(&root.join("usr/share/factory/etc"), "", &mut files)?;
            digest(format!("{:?}", files).as_bytes())
        };
    reconcile(root, generation.trim(), &mut || Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("etc-layout-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            for name in ["etc", "usr/share/factory/etc", "tmp", "run"] { fs::create_dir_all(root.join(name)).unwrap(); }
            Self(root)
        }
        fn put(&self, name: &str, bytes: &[u8]) { fs::write(self.0.join(name), bytes).unwrap(); }
        fn factory(&self, name: &str, bytes: &[u8]) { self.put(&format!("usr/share/factory/etc/{}", name), bytes); }
        fn read(&self, name: &str) -> Vec<u8> { fs::read(self.0.join(name)).unwrap() }
        fn apply(&self, id: &str) { reconcile(&self.0, id, &mut || Ok(())).unwrap(); }
    }
    impl Drop for Tree { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }

    fn upgrade_tree() -> Tree {
        let tree = Tree::new();
        for name in ["unchanged", "local", "gone"] { tree.factory(name, b"old"); }
        tree.factory("passwd", b"root:x:0:0::/root:/bin/sh\n");
        tree.apply("old");
        tree.put("etc/local", b"operator edit\0\xff");
        tree.put("etc/passwd", b"root:x:0:0:local:/root:/bin/sh\noperator:x:1000:1000::/home/operator:/bin/sh\n");
        tree.factory("unchanged", b"new"); tree.factory("local", b"new");
        fs::remove_file(tree.0.join("usr/share/factory/etc/gone")).unwrap();
        tree.factory("added", b"new");
        tree
    }

    fn assert_upgrade(tree: &Tree) {
        assert_eq!(tree.read("etc/unchanged"), b"new");
        assert_eq!(tree.read("etc/local"), b"operator edit\0\xff");
        assert_eq!(tree.read("etc/local.factory"), b"new");
        assert_eq!(tree.read("etc/added"), b"new");
        assert!(!tree.0.join("etc/gone").exists());
        assert_eq!(tree.read("etc/passwd"), b"root:x:0:0:local:/root:/bin/sh\noperator:x:1000:1000::/home/operator:/bin/sh\n");
        assert_eq!(parse_record(&tree.read("etc/.breenix-factory")).unwrap().0, "new");
    }

    #[test]
    fn first_v2_boot_replaces_only_exact_legacy_defaults() {
        let tree = Tree::new();
        for name in ["bshrc", "hotkeys.conf"] {
            tree.put(&format!("etc/{}", name), legacy_default(name).unwrap());
            tree.factory(name, b"new locations");
        }
        tree.apply("v2");
        for name in ["bshrc", "hotkeys.conf"] {
            assert_eq!(tree.read(&format!("etc/{}", name)), b"new locations");
            // An earlier v2 boot may have recorded new defaults but kept v1 bytes.
            tree.put(&format!("etc/{}", name), legacy_default(name).unwrap());
        }
        tree.apply("v2-repair");
        for name in ["bshrc", "hotkeys.conf"] {
            assert_eq!(tree.read(&format!("etc/{}", name)), b"new locations");
        }
        let edited = Tree::new();
        edited.put("etc/bshrc", b"local PATH settings");
        edited.factory("bshrc", b"new locations");
        edited.apply("v2");
        assert_eq!(edited.read("etc/bshrc"), b"local PATH settings");
        assert_eq!(edited.read("etc/bshrc.factory"), b"new locations");
    }

    #[test]
    fn upgrade_and_rollback_preserve_edits_and_machine_accounts() {
        let tree = upgrade_tree(); tree.apply("new"); assert_upgrade(&tree);
        tree.factory("unchanged", b"old"); tree.factory("local", b"old");
        tree.apply("old");
        assert_eq!(tree.read("etc/unchanged"), b"old");
        assert_eq!(tree.read("etc/local"), b"operator edit\0\xff");
        assert_eq!(tree.read("etc/local.factory"), b"old");
        assert!(String::from_utf8(tree.read("etc/passwd")).unwrap().contains("operator:x:1000"));
    }

    #[test]
    fn interruption_at_each_operation_recovers_without_losing_local_edits() {
        let probe = upgrade_tree();
        let mut count = 0;
        reconcile(&probe.0, "new", &mut || { count += 1; Ok(()) }).unwrap();
        for fault in 0..count {
            let tree = upgrade_tree();
            let mut left = fault;
            assert!(reconcile(&tree.0, "new", &mut || {
                if left == 0 { return Err("injected I/O failure".into()); }
                left -= 1; Ok(())
            }).is_err(), "fault={fault}");
            tree.apply("new"); assert_upgrade(&tree);
        }
    }

    #[test]
    fn local_symlink_is_kept_and_factory_parent_symlink_is_refused() {
        let tree = Tree::new();
        tree.factory("config", b"factory");
        tree.put("outside", b"untouched");
        std::os::unix::fs::symlink("../outside", tree.0.join("etc/config")).unwrap();
        tree.apply("a");
        assert!(fs::symlink_metadata(tree.0.join("etc/config")).unwrap().is_symlink());
        assert_eq!(tree.read("outside"), b"untouched");
        assert_eq!(tree.read("etc/config.factory"), b"factory");
        fs::create_dir(tree.0.join("usr/share/factory/etc/sub")).unwrap();
        tree.factory("sub/file", b"refuse");
        std::os::unix::fs::symlink("..", tree.0.join("etc/sub")).unwrap();
        assert!(reconcile(&tree.0, "b", &mut || Ok(())).is_err());
    }

    #[test]
    fn startup_clears_run_and_tmp_but_keeps_var() {
        let tree = Tree::new();
        fs::create_dir_all(tree.0.join("var/tmp")).unwrap();
        for path in ["tmp/stale", "run/stale", "var/tmp/keep"] { tree.put(path, b"sentinel"); }
        clear_volatile(&tree.0).unwrap();
        assert!(!tree.0.join("run/stale").exists() && !tree.0.join("tmp/stale").exists());
        assert_eq!(tree.read("var/tmp/keep"), b"sentinel");
    }
}
