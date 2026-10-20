//! Stage a userland beside the running install, then publish it with one rename.
//! The kernel slot embeds the generation ID, so no independently mutable current
//! pointer can pair a kernel with the wrong userland. `/etc` belongs to the
//! generation; `/root` stays shared. This program mounts no home partition.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TARGET: &str = "/home";
const KEPT: [&str; 1] = ["root"];
const UNTOUCHED: [&str; 7] = ["home", "lost+found", "proc", "dev", "sys", ".breenix-generations", ".complete"];
const GENERATIONS: &str = ".breenix-generations";
const READY: &str = ".complete";

#[derive(Default)]
struct Totals {
    files: u64,
    dirs: u64,
    links: u64,
    hard_links: u64,
    bytes: u64,
    left_out: u64,
}

/// Whether the kernel can act on `name` by name: its path system calls refuse names that are not UTF-8.
fn usable(name: &std::ffi::OsStr, parent: &Path, totals: &mut Totals) -> bool {
    if name.to_str().is_some() {
        return true;
    }
    println!("[persistent] left out {}: the name is not UTF-8", parent.join(name).display());
    totals.left_out += 1;
    false
}

struct Copier<'a> {
    totals: Totals,
    /// The first copy of each multiply-linked file, by its source inode, so its other names become hard links.
    linked: HashMap<(u64, u64), PathBuf>,
    buffer: Vec<u8>,
    checkpoint: &'a mut dyn FnMut() -> std::io::Result<()>,
}

fn fail(what: &str, path: &Path, error: std::io::Error) -> String {
    format!("{} {}: {}", what, path.display(), error)
}

fn own(path: &Path, meta: &fs::Metadata, follow: bool) -> std::io::Result<()> {
    let result = if follow {
        std::os::unix::fs::chown(path, Some(meta.uid()), Some(meta.gid()))
    } else {
        std::os::unix::fs::lchown(path, Some(meta.uid()), Some(meta.gid()))
    };
    result
}

impl Copier<'_> {
    /// Tests fail before and after each persisted operation, including writes
    /// that may have completed before reporting an error.
    fn change<T>(&mut self, what: &str, path: &Path, op: impl FnOnce() -> std::io::Result<T>) -> Result<T, String> {
        (self.checkpoint)().map_err(|error| fail(what, path, error))?;
        let result = op().map_err(|error| fail(what, path, error))?;
        (self.checkpoint)().map_err(|error| fail(what, path, error))?;
        Ok(result)
    }

    fn sync(&mut self, path: &Path) -> Result<(), String> {
        self.change("sync", path, || File::open(path)?.sync_all())
    }

    fn copy_tree(&mut self, source: &Path, target: &Path) -> Result<(), String> {
        let meta = fs::symlink_metadata(source).map_err(|error| fail("stat", source, error))?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            let to = fs::read_link(source).map_err(|error| fail("readlink", source, error))?;
            self.change("symlink", target, || std::os::unix::fs::symlink(&to, target))?;
            self.change("chown", target, || own(target, &meta, false))?;
            self.totals.links += 1;
        } else if kind.is_dir() {
            self.change("mkdir", target, || fs::create_dir(target))?;
            let mut names: Vec<_> = fs::read_dir(source)
                .map_err(|error| fail("read directory", source, error))?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<Result<_, _>>()
                .map_err(|error| fail("read directory", source, error))?;
            names.sort();
            for name in names {
                if usable(&name, source, &mut self.totals) {
                    self.copy_tree(&source.join(&name), &target.join(&name))?;
                }
            }
            self.change("chown", target, || own(target, &meta, true))?;
            self.change("chmod", target, || fs::set_permissions(target, fs::Permissions::from_mode(meta.mode() & 0o7777)))?;
            self.sync(target)?;
            self.totals.dirs += 1;
        } else if kind.is_file() {
            let key = (meta.dev(), meta.ino());
            if meta.nlink() > 1 {
                if let Some(first) = self.linked.get(&key) {
                    let first = first.clone();
                    self.change("link", target, || fs::hard_link(&first, target))?;
                    self.totals.hard_links += 1;
                    return Ok(());
                }
                self.linked.insert(key, target.to_path_buf());
            }
            let mut input = File::open(source).map_err(|error| fail("open", source, error))?;
            let mut output = self.change("create", target, || File::create(target))?;
            loop {
                let count = input.read(&mut self.buffer).map_err(|error| fail("read", source, error))?;
                if count == 0 {
                    break;
                }
                (self.checkpoint)().map_err(|error| fail("write", target, error))?;
                output.write_all(&self.buffer[..count]).map_err(|error| fail("write", target, error))?;
                (self.checkpoint)().map_err(|error| fail("write", target, error))?;
                self.totals.bytes += count as u64;
            }
            self.change("sync file", target, || output.sync_all())?;
            drop(output);
            self.change("chown", target, || own(target, &meta, true))?;
            self.change("chmod", target, || fs::set_permissions(target, fs::Permissions::from_mode(meta.mode() & 0o7777)))?;
            self.sync(target)?;
            self.totals.files += 1;
        } else {
            // Devices, FIFOs and sockets belong to the running system, not to the userland on disk.
            println!("[persistent] left out {}: not a file, directory or link", source.display());
        }
        Ok(())
    }
}

fn present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fail("stat", path, error)),
    }
}

fn require_directory(path: &Path) -> Result<(), String> {
    if !fs::symlink_metadata(path).map_err(|error| fail("stat", path, error))?.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(())
}

fn install_tree(source: &Path, target: &Path, id: &str, checkpoint: &mut dyn FnMut() -> std::io::Result<()>) -> Result<Totals, String> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(String::from("invalid userland generation"));
    }
    require_directory(target)?;
    let generations = target.join(GENERATIONS);
    let staged = generations.join(format!("{}.new", id));
    let published = generations.join(id);
    if present(&published)? {
        require_directory(&published)?;
        let expected = format!("BREENIX-USERLAND 1\n{}\n", id);
        let ready = published.join(READY);
        if fs::symlink_metadata(&ready).map_err(|error| fail("stat", &ready, error))?.is_file()
            && fs::read(&ready).map_err(|error| fail("read", &ready, error))? == expected.as_bytes() {
            for name in ["etc", "root", "home", "dev", "proc", "sys"] {
                require_directory(&published.join(name))?;
            }
            require_directory(&target.join("root"))?;
            return Ok(Totals::default());
        }
        return Err(String::from("incomplete published userland generation"));
    }
    for name in KEPT {
        require_directory(&source.join(name))?;
        if present(&target.join(name))? { require_directory(&target.join(name))?; }
    }
    let mut copier = Copier { totals: Totals::default(), linked: HashMap::new(), buffer: vec![0u8; 64 * 1024], checkpoint };
    if !present(&generations)? {
        copier.change("mkdir", &generations, || fs::create_dir(&generations))?;
        copier.sync(target)?;
    }
    require_directory(&generations)?;
    // Boot IDs change on retry. Reclaim scratch from any unpublished ID, but
    // retain final generations and shared scratch: a torn shared rename can
    // leave a second name for live operator data.
    for entry in fs::read_dir(&generations).map_err(|error| fail("read directory", &generations, error))? {
        let entry = entry.map_err(|error| fail("read directory", &generations, error))?;
        let name = entry.file_name();
        if let Some(scratch_id) = name.to_str().and_then(|name| name.strip_suffix(".new")) {
            if !scratch_id.is_empty() && scratch_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !present(&generations.join(scratch_id))? {
                let scratch = entry.path();
                copier.change("remove scratch", &scratch, || fs::remove_dir_all(&scratch))?;
            }
        }
    }
    copier.change("mkdir", &staged, || fs::create_dir(&staged))?;
    let mut names = Vec::new();
    for entry in fs::read_dir(source).map_err(|error| fail("read directory", source, error))? {
        let name = entry.map_err(|error| fail("read directory", source, error))?.file_name();
        if usable(&name, source, &mut copier.totals) {
            names.push(name.into_string().unwrap_or_default());
        }
    }
    names.sort();
    for name in names.iter().filter(|name| !UNTOUCHED.contains(&name.as_str()) && !KEPT.contains(&name.as_str())) {
        copier.copy_tree(&source.join(name), &staged.join(name))?;
    }
    // Shared directories are copied privately when absent, then renamed into
    // place only after the copy is durable. Existing directories are kept
    // during upgrades, including upgrades from the old flat layout.
    for name in KEPT {
        let destination = target.join(name);
        if !present(&destination)? {
            // Keep shared scratch outside the generation: an interrupted
            // cross-directory rename can leave both source and destination
            // naming it. Retrying must not recurse through that alias when
            // removing the generation's unpublished scratch tree.
            let scratch = generations.join(format!("{}.shared-{}", id, name));
            if present(&scratch)? {
                copier.change("remove unpublished shared scratch", &scratch, || fs::remove_dir_all(&scratch))?;
            }
            // Keep hard-link groups inside each independently published tree.
            copier.linked.clear();
            copier.copy_tree(&source.join(name), &scratch)?;
            copier.sync(&generations)?;
            copier.change("publish shared directory", &destination, || fs::rename(&scratch, &destination))?;
            copier.sync(target)?;
            copier.sync(&generations)?;
        }
    }
    for name in ["root", "home", "dev", "proc", "sys"] {
        let mount = staged.join(name);
        copier.change("mkdir mount", &mount, || fs::create_dir(&mount))?;
    }
    let ready = staged.join(READY);
    let record = format!("BREENIX-USERLAND 1\n{}\n", id);
    let mut file = copier.change("create completion record", &ready, || File::create(&ready))?;
    copier.change("write completion record", &ready, || file.write_all(record.as_bytes()))?;
    copier.change("sync completion record", &ready, || file.sync_all())?;
    let root_meta = fs::symlink_metadata(source).map_err(|error| fail("stat", source, error))?;
    copier.change("chown root", &staged, || own(&staged, &root_meta, true))?;
    copier.change("chmod root", &staged, || fs::set_permissions(&staged, fs::Permissions::from_mode(root_meta.mode() & 0o7777)))?;
    copier.sync(&staged)?;
    copier.change("publish generation", &published, || fs::rename(&staged, &published))?;
    copier.sync(&generations)?;
    Ok(copier.totals)
}

fn install() -> Result<Totals, String> {
    let source = Path::new("/");
    let target = Path::new(TARGET);
    let root_dev = fs::metadata(source).map_err(|error| fail("stat", source, error))?.dev();
    let target_dev = fs::metadata(target).map_err(|error| fail("stat", target, error))?.dev();
    if root_dev == target_dev { return Err(String::from("the system partition is not mounted at /home")); }
    let id = std::env::args().nth(1).ok_or("install intent names no userland generation")?;
    install_tree(source, target, &id, &mut || Ok(()))
}

fn main() {
    println!("[persistent] install starting: the test root's userland onto the system partition at /home");
    match install() {
        Ok(totals) => {
            println!(
                "[persistent] install complete: files={} dirs={} links={} hard-links={} bytes={} left-out={}",
                totals.files, totals.dirs, totals.links, totals.hard_links, totals.bytes, totals.left_out
            );
        }
        Err(why) => println!("[persistent] install failed: {}", why),
    }
    // PID 1 does not exit; the boot loop powers the machine off once it has read the result.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg(test)]
extern crate alloc;
#[cfg(test)]
#[path = "../../../kernel/src/fs/ext2/generation.rs"]
mod generation;
#[cfg(test)]
#[path = "../../../scripts/metal/efi-stub/src/slot_transaction.rs"]
mod slots;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("userland-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn source(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir(&path).unwrap();
            for directory in ["bin", "sbin", "etc", "root", "home", "dev", "proc", "sys"] {
                fs::create_dir(path.join(directory)).unwrap();
            }
            fs::write(path.join("bin/payload"), name.as_bytes()).unwrap();
            fs::set_permissions(path.join("bin/payload"), fs::Permissions::from_mode(0o6755)).unwrap();
            fs::hard_link(path.join("bin/payload"), path.join("sbin/init")).unwrap();
            std::os::unix::fs::symlink("../bin/payload", path.join("sbin/link")).unwrap();
            fs::write(path.join("etc/config"), name.as_bytes()).unwrap();
            fs::write(path.join("root/data"), b"source data").unwrap();
            path
        }
        fn system(&self) -> PathBuf {
            let path = self.source("system");
            fs::write(path.join("etc/config"), b"operator config").unwrap();
            fs::write(path.join("root/data"), b"operator data\0\xff").unwrap();
            fs::write(path.join("home/untouched"), b"home mount point data").unwrap();
            // The installer reaches the home mount point through the target.
            // Put the stand-in filesystem there so an erroneous recursive copy
            // or removal can actually damage it.
            fs::write(path.join("home/data"), b"operator home\0\xff").unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
    }

    // Include bytes, modes, ownership, symlink targets and link groups. Ignore
    // access times, which a read itself may update.
    fn snapshot(path: &Path) -> BTreeMap<PathBuf, (u32, u32, u32, Vec<u8>, Option<u64>)> {
        fn walk(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, (u32, u32, u32, Vec<u8>, Option<u64>)>) {
            let meta = fs::symlink_metadata(path).unwrap();
            let bytes = if meta.is_file() { fs::read(path).unwrap() }
                else if meta.is_symlink() { fs::read_link(path).unwrap().as_os_str().as_encoded_bytes().to_vec() }
                else { Vec::new() };
            result.insert(path.strip_prefix(root).unwrap().into(),
                (meta.mode(), meta.uid(), meta.gid(), bytes, meta.is_file().then_some(meta.ino())));
            if meta.is_dir() {
                for entry in fs::read_dir(path).unwrap() { walk(root, &entry.unwrap().path(), result); }
            }
        }
        let mut result = BTreeMap::new();
        walk(path, path, &mut result);
        result
    }

    /// Adapter over real host directories for the kernel's production selection
    /// code. Inode numbers here are assigned handles independent of host inode values.
    struct Tree(RefCell<BTreeMap<u32, PathBuf>>);
    impl Tree {
        fn new(system: &Path) -> Self { Self(RefCell::new(BTreeMap::from([(2, system.into())]))) }
        fn path(&self, ino: u32) -> PathBuf { self.0.borrow()[&ino].clone() }
    }
    impl generation::Tree for Tree {
        fn directory(&self, parent: u32, name: &str) -> Result<u32, &'static str> {
            let path = self.path(parent).join(name);
            if !fs::symlink_metadata(&path).map_err(|_| "stat")?.is_dir() { return Err("not directory"); }
            let mut paths = self.0.borrow_mut();
            if let Some((&ino, _)) = paths.iter().find(|(_, value)| **value == path) { return Ok(ino); }
            let ino = paths.len() as u32 + 2;
            paths.insert(ino, path);
            Ok(ino)
        }
        fn contents(&self, parent: u32, name: &str) -> Result<Vec<u8>, &'static str> {
            let path = self.path(parent).join(name);
            let meta = fs::symlink_metadata(&path).map_err(|_| "stat")?;
            if !meta.is_file() || meta.len() > 128 { return Err("not record"); }
            fs::read(path).map_err(|_| "read")
        }
    }

    fn selected(system: &Path, id: &str) -> PathBuf {
        assert!(generation::Namespace::RAW.is_mount(2));
        let tree = Tree::new(system);
        let namespace = generation::select(&tree, id).unwrap();
        assert!(namespace.is_mount(namespace.root));
        assert_eq!(tree.path(namespace.shared(namespace.root, b"etc").unwrap()), system.join(GENERATIONS).join(id).join("etc"));
        assert_eq!(tree.path(namespace.shared(namespace.root, b"root").unwrap()), system.join("root"));
        assert_eq!(namespace.shared(namespace.root, b"home"), None);
        assert!(namespace.is_mount(namespace.etc));
        tree.path(namespace.root)
    }

    #[test]
    fn interruption_enospc_and_io_error_at_each_operation_keep_current_bootable() {
        for flat in [false, true] {
            let baseline = Fixture::new();
            let source = baseline.source("new");
            let system = baseline.system();
            if !flat {
                install_tree(&baseline.source("old"), &system, "old", &mut || Ok(())).unwrap();
            }
            let mut checkpoints = 0;
            install_tree(&source, &system, "count", &mut || { checkpoints += 1; Ok(()) }).unwrap();
            for errno in [4, 5, 28] {
                for fault in 0..checkpoints {
                    let fixture = Fixture::new();
                    let source = fixture.source("new");
                    let system = fixture.system();
                    let current = if flat { system.clone() } else {
                        let old = fixture.source("old");
                        install_tree(&old, &system, "old", &mut || Ok(())).unwrap();
                        selected(&system, "old")
                    };
                    let old_bin = snapshot(&current.join("bin"));
                    let old_sbin = snapshot(&current.join("sbin"));
                    let data: Vec<_> = [system.join("etc"), system.join("root"), system.join("home")]
                        .iter().map(|path| (path.clone(), snapshot(path))).collect();
                    let mut left = fault;
                    let result = install_tree(&source, &system, "new", &mut || {
                        if left == 0 { return Err(std::io::Error::from_raw_os_error(errno)); }
                        left -= 1;
                        Ok(())
                    });
                    assert!(result.is_err(), "flat={flat} errno={errno} fault={fault}");
                    if !flat { assert_eq!(selected(&system, "old"), current); }
                    assert_eq!(snapshot(&current.join("bin")), old_bin);
                    assert_eq!(snapshot(&current.join("sbin")), old_sbin);
                    for (path, before) in data { assert_eq!(snapshot(&path), before); }
                    // A reset after the final rename may leave a completed,
                    // unselected generation. It must be bootable too.
                    if system.join(GENERATIONS).join("new").exists() {
                        let root = selected(&system, "new");
                        assert_eq!(fs::read(root.join("bin/payload")).unwrap(), b"new");
                    }
                }
            }
        }
    }

    #[test]
    fn retry_with_new_boot_id_reclaims_only_unpublished_scratch() {
        let fixture = Fixture::new();
        let system = fixture.system();
        let source = fixture.source("old");
        install_tree(&source, &system, "old", &mut || Ok(())).unwrap();
        let generations = system.join(GENERATIONS);
        fs::create_dir(generations.join("failed.new")).unwrap();
        fs::write(generations.join("failed.new/partial"), vec![0u8; 65536]).unwrap();
        // Model second names left after publication; retain both aliases.
        std::os::unix::fs::symlink(system.join("root"), generations.join("old.shared-root")).unwrap();
        std::os::unix::fs::symlink(generations.join("old"), generations.join("old.new")).unwrap();
        install_tree(&fixture.source("new"), &system, "new", &mut || Ok(())).unwrap();
        assert!(!generations.join("failed.new").exists());
        assert!(generations.join("old.shared-root").exists());
        assert!(generations.join("old.new").exists());
        assert_eq!(fs::read(selected(&system, "old").join("etc/config")).unwrap(), b"old");
        assert_eq!(fs::read(selected(&system, "new").join("etc/config")).unwrap(), b"new");
        assert_eq!(fs::read(system.join("root/data")).unwrap(), b"operator data\0\xff");
    }

    #[test]
    fn generation_root_keeps_source_mode_after_writing_completion() {
        let fixture = Fixture::new();
        let source = fixture.source("new");
        let system = fixture.system();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
        install_tree(&source, &system, "new", &mut || Ok(())).unwrap();
        let root = selected(&system, "new");
        assert_eq!(fs::metadata(&root).unwrap().mode() & 0o7777, 0o555);
        assert_eq!(fs::metadata(&root).unwrap().uid(), fs::metadata(&source).unwrap().uid());
        assert_eq!(fs::metadata(&root).unwrap().gid(), fs::metadata(&source).unwrap().gid());
        // Restore write access for fixture cleanup on unprivileged hosts.
        fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(source, fs::Permissions::from_mode(0o755)).unwrap();
    }

    struct SlotDisk {
        files: RefCell<BTreeMap<String, Vec<u8>>>,
        left: Cell<usize>,
        after: bool,
    }
    impl SlotDisk {
        fn change(&self, operation: impl FnOnce(&mut BTreeMap<String, Vec<u8>>)) -> Result<(), ()> {
            let count = self.left.get();
            if count == 0 && !self.after { return Err(()); }
            operation(&mut self.files.borrow_mut());
            if count == 0 { return Err(()); }
            self.left.set(count - 1);
            Ok(())
        }
        fn id(&self, slot: &str) -> String {
            String::from_utf8(self.files.borrow()[&format!("{}.IMG", slot)].clone()).unwrap()
        }
    }
    impl slots::Store for SlotDisk {
        type Error = ();
        fn exists(&self, name: &str) -> bool { self.files.borrow().contains_key(name) }
        fn read(&self, name: &str, out: &mut [u8]) -> Result<usize, ()> {
            let files = self.files.borrow();
            let bytes = files.get(name).ok_or(())?;
            if bytes.len() > out.len() { return Err(()); }
            out[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        }
        fn write(&self, name: &str, bytes: &[u8]) -> Result<(), ()> {
            self.change(|files| { files.insert(name.into(), bytes.into()); })
        }
        fn delete(&self, name: &str) -> Result<(), ()> {
            self.change(|files| { files.remove(name); })
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), ()> {
            assert!(!self.files.borrow().contains_key(to));
            self.change(|files| { let bytes = files.remove(from).unwrap(); files.insert(to.into(), bytes); })
        }
        fn invalid(&self) {}
    }

    #[test]
    fn promotion_and_rollback_recover_paired_trees_byte_for_byte() {
        let fixture = Fixture::new();
        let system = fixture.system();
        let mut recorded = BTreeMap::new();
        for id in ["old", "new"] {
            let source = fixture.source(id);
            // Record the source before installation, independently of whatever
            // selection later returns. Compare ownership available to this host:
            // unprivileged tests cannot chown to arbitrary uid/gid values.
            recorded.insert(id, snapshot(&source));
            install_tree(&source, &system, id, &mut || Ok(())).unwrap();
        }
        let old = &recorded["old"];
        let new = &recorded["new"];
        let assert_contents = |root: &Path, expected: &BTreeMap<PathBuf, (u32, u32, u32, Vec<u8>, Option<u64>)>| {
            let actual = snapshot(root);
            let contents = |tree: &BTreeMap<PathBuf, (u32, u32, u32, Vec<u8>, Option<u64>)>| {
                tree.iter().filter(|(path, _)| {
                    !["root", "home", "dev", "proc", "sys", READY].iter().any(|name| path.starts_with(name))
                }).map(|(path, (mode, uid, gid, bytes, _))| {
                    (path.clone(), (*mode, *uid, *gid, bytes.clone()))
                }).collect::<BTreeMap<_, _>>()
            };
            assert_eq!(contents(&actual), contents(expected));
            assert_eq!(actual[Path::new("bin/payload")].4, actual[Path::new("sbin/init")].4);
        };
        let initial: BTreeMap<_, _> = [("CURRENT.IMG", b"old".to_vec()), ("CURRENT.TXT", b"old".to_vec()),
            ("NEXT.IMG", b"new".to_vec()), ("NEXT.TXT", b"new".to_vec())].into_iter().map(|(k,v)| (k.into(),v)).collect();
        for rollback in [false, true] {
            for after in [false, true] {
                let probe = SlotDisk { files: RefCell::new(initial.clone()), left: Cell::new(usize::MAX), after };
                if rollback { slots::begin(&probe, false).unwrap(); }
                let start = probe.files.borrow().clone();
                probe.left.set(usize::MAX);
                slots::begin(&probe, rollback).unwrap();
                let operations = usize::MAX - probe.left.get();
                for fault in 0..=operations {
                    let disk = SlotDisk { files: RefCell::new(start.clone()), left: Cell::new(fault), after };
                    let _ = slots::begin(&disk, rollback);
                    disk.left.set(usize::MAX);
                    slots::recover(&disk).unwrap();
                    let id = disk.id("CURRENT");
                    let selected_root = selected(&system, &id);
                    assert_eq!(fs::read(selected_root.join("bin/payload")).unwrap(), id.as_bytes());
                    assert_eq!(fs::read(selected_root.join("etc/config")).unwrap(), id.as_bytes());
                    let expected = if id == "old" { old } else { assert_eq!(id, "new"); new };
                    assert_contents(&selected_root, expected);
                    assert_eq!(disk.files.borrow()["CURRENT.TXT"], id.as_bytes());
                    // Complete promotion then rollback without a fault: the
                    // actual slot transaction must return exactly the old tree.
                    if !rollback && id == "new" {
                        slots::begin(&disk, true).unwrap();
                        assert_eq!(disk.id("CURRENT"), "old");
                        assert_contents(&selected(&system, &disk.id("CURRENT")), old);
                    }
                }
            }
        }
        assert_eq!(fs::read(system.join("root/data")).unwrap(), b"operator data\0\xff");
        assert_eq!(fs::read(system.join("home/data")).unwrap(), b"operator home\0\xff");
    }

    #[test]
    fn first_install_retry_keeps_published_shared_data_at_each_checkpoint() {
        let baseline = Fixture::new();
        let source = baseline.source("new");
        let target = baseline.0.join("blank");
        fs::create_dir(&target).unwrap();
        let mut count = 0;
        install_tree(&source, &target, "first", &mut || { count += 1; Ok(()) }).unwrap();
        for fault in 0..count {
            let fixture = Fixture::new();
            let source = fixture.source("new");
            let target = fixture.0.join("blank");
            fs::create_dir(&target).unwrap();
            let mut left = fault;
            assert!(install_tree(&source, &target, "first", &mut || {
                if left == 0 { return Err(std::io::Error::from_raw_os_error(5)); }
                left -= 1;
                Ok(())
            }).is_err());
            let mut data = Vec::new();
            for name in KEPT {
                let path = target.join(name);
                if path.exists() {
                    fs::write(path.join("operator-file"), b"written after interruption").unwrap();
                    data.push((path.clone(), snapshot(&path)));
                }
            }
            install_tree(&source, &target, "first", &mut || Ok(())).unwrap();
            assert_eq!(fs::read(selected(&target, "first").join("bin/payload")).unwrap(), b"new");
            for (path, before) in data { assert_eq!(snapshot(&path), before); }
        }
    }

    #[test]
    fn incomplete_generations_are_refused_and_first_install_resumes() {
        let fixture = Fixture::new();
        let source = fixture.source("new");
        let system = fixture.0.join("blank");
        fs::create_dir(&system).unwrap();
        let mut checkpoints = 0;
        // Stop inside first-install copying, then retry the same scratch ID.
        assert!(install_tree(&source, &system, "first", &mut || {
            checkpoints += 1;
            if checkpoints == 30 { Err(std::io::Error::from_raw_os_error(5)) } else { Ok(()) }
        }).is_err());
        assert!(generation::select(&Tree::new(&system), "first").is_err());
        install_tree(&source, &system, "first", &mut || Ok(())).unwrap();
        let root = selected(&system, "first");
        assert_eq!(fs::read(root.join("bin/payload")).unwrap(), b"new");
        assert_eq!(fs::metadata(root.join("bin/payload")).unwrap().ino(), fs::metadata(root.join("sbin/init")).unwrap().ino());
        assert_eq!(fs::metadata(root.join("bin/payload")).unwrap().mode() & 0o7777, 0o6755);
        let before = snapshot(&system);
        install_tree(&source, &system, "first", &mut || panic!("mutated published tree")).unwrap();
        assert_eq!(snapshot(&system), before);
        for id in ["../first", "first.new", "", ".", ".."] {
            assert!(generation::select(&Tree::new(&system), id).is_err());
            assert!(install_tree(&source, &system, id, &mut || panic!("invalid ID mutated disk")).is_err());
        }
        fs::write(root.join(READY), b"wrong generation\n").unwrap();
        assert!(generation::select(&Tree::new(&system), "first").is_err());
        assert!(install_tree(&source, &system, "first", &mut || panic!("mutated incomplete publication")).is_err());
    }
}
