#![allow(dead_code)] // Pulls in the standalone installer, including its guest-only entry points.
//! DEV-52(2b): rehearse the paired deploy on the host. The test drives the installer's
//! own subcommand dispatcher (`Session::run`, the code `persistent-install paired` runs on
//! the box) over a directory model of the BreenFS writer: roots are directories, a clone
//! copies the booted root with its hard links, owners, modes and mtimes, and the epoch is
//! a digest of the booted tree. The inventory and the object set come from the host's own
//! `prepare_inventory` and `export_objects` (paired_image.py).
//!
//! `BREENIX_PAIRED_IMAGE=<ext2.img>` runs the rehearsal on that image, the `ext2.img` that
//! metal-build delivers. Without it, the real-image case reports itself skipped and the
//! committed fixture (paired_image.py `fixture`) runs the same rehearsal.
//!
//! Linux, as root: the image's names (byte 0xff) and owners (root, set-ID) must be
//! reproduced exactly; APFS refuses such names. tests/persistent_install/linux.sh runs this
//! in a container.
#[path = "../../userspace/programs/src/persistent_install.rs"]
mod installer;

#[cfg(target_os = "linux")]
mod rehearsal {
    use super::installer::paired::{
        digest,
        session::{Places, Roots, Session, Writer, PROTOCOL},
    };
    use std::{
        cell::{Cell, RefCell},
        collections::{BTreeMap, BTreeSet, HashMap},
        ffi::{CString, OsStr},
        fs,
        hash::{DefaultHasher, Hasher},
        io::Read,
        os::unix::{
            ffi::OsStrExt,
            fs::{lchown, symlink, MetadataExt, PermissionsExt},
        },
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

    const OLD: &str = "20261128T010000Z-metal";
    const PREVIOUS: &str = "20261127T230000Z-metal";
    const ID1: &str = "20261128T020000Z-metal";
    const ID2: &str = "20261128T030000Z-metal";
    const ID3: &str = "20261128T040000Z-metal";
    const ID4: &str = "20261128T050000Z-metal";
    const ID5: &str = "20261128T060000Z-metal";
    const DEBRIS: &str = "20261127T220000Z-metal";
    const TOOLCHAIN: &str = "rehearsal-toolchain";

    fn helper(args: &[&OsStr], input: Option<&[u8]>) {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("paired_image.py");
        let mut child = Command::new("python3")
            .arg(script)
            .args(args)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(input.unwrap_or_default()).unwrap();
        }
        assert!(
            child.wait().unwrap().success(),
            "paired_image.py {args:?} failed"
        );
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn rel(path: &[u8]) -> &Path {
        Path::new(OsStr::from_bytes(&path[1..]))
    }

    #[derive(Clone)]
    struct Row {
        path: Vec<u8>,
        kind: String,
        mode: u32,
        uid: u32,
        gid: u32,
        group: Option<Vec<u8>>,
        size: u64,
        mtime: (i64, i64),
        sha: String,
    }

    fn rows(inventory: &[u8]) -> Vec<Row> {
        std::str::from_utf8(inventory)
            .unwrap()
            .lines()
            .map(|line| {
                let f: Vec<_> = line.split('\t').collect();
                let (sec, ns) = f[7].split_once('.').unwrap();
                Row {
                    path: unhex(f[0]),
                    kind: f[1].into(),
                    mode: u32::from_str_radix(f[2], 8).unwrap(),
                    uid: f[3].parse().unwrap(),
                    gid: f[4].parse().unwrap(),
                    group: (f[5] != "-").then(|| unhex(f[5])),
                    size: f[6].parse().unwrap(),
                    mtime: (sec.parse().unwrap(), ns.parse().unwrap()),
                    sha: f[8].into(),
                }
            })
            .collect()
    }

    fn set_mtime(path: &Path, (sec, ns): (i64, i64)) {
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        let times = [
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
            libc::timespec {
                tv_sec: sec,
                tv_nsec: ns,
            },
        ];
        let rc = unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                name.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        assert_eq!(rc, 0, "utimensat {path:?}");
    }

    fn owner_mode(path: &Path, uid: u32, gid: u32, mode: u32) {
        lchown(path, Some(uid), Some(gid)).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Install an inventory into an empty root the way a persistent install leaves it.
    /// Independent of the installer under test.
    fn materialize(root: &Path, rows: &[Row], objects: &Path) {
        for row in rows {
            let path = root.join(rel(&row.path));
            match (row.kind.as_str(), &row.group) {
                ("dir", _) => {
                    fs::create_dir_all(&path).unwrap();
                    owner_mode(&path, row.uid, row.gid, row.mode);
                }
                ("reg", Some(first)) if *first != row.path => {
                    fs::hard_link(root.join(rel(first)), &path).unwrap()
                }
                ("reg", _) => {
                    fs::copy(objects.join(&row.sha), &path).unwrap();
                    owner_mode(&path, row.uid, row.gid, row.mode);
                }
                ("symlink", _) => {
                    symlink(
                        OsStr::from_bytes(&fs::read(objects.join(&row.sha)).unwrap()),
                        &path,
                    )
                    .unwrap();
                    lchown(&path, Some(row.uid), Some(row.gid)).unwrap();
                }
                (kind, _) => panic!("unexpected inventory kind {kind}"),
            }
        }
        for row in rows.iter().rev() {
            set_mtime(&root.join(rel(&row.path)), row.mtime);
        }
    }

    /// Copy a tree as a BreenFS clone shares it: same names, owners, modes, mtimes and
    /// hard-link structure.
    fn clone_tree(from: &Path, to: &Path) {
        fn walk(
            from: &Path,
            to: &Path,
            links: &mut HashMap<u64, PathBuf>,
            dirs: &mut Vec<(PathBuf, fs::Metadata)>,
        ) {
            let meta = fs::symlink_metadata(from).unwrap();
            if meta.is_dir() {
                fs::create_dir(to).unwrap();
                let mut names: Vec<_> = fs::read_dir(from)
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                names.sort();
                for name in names {
                    walk(&from.join(&name), &to.join(&name), links, dirs);
                }
                dirs.push((to.to_path_buf(), meta));
                return;
            }
            if meta.file_type().is_symlink() {
                symlink(fs::read_link(from).unwrap(), to).unwrap();
                lchown(to, Some(meta.uid()), Some(meta.gid())).unwrap();
            } else if let Some(first) = links.get(&meta.ino()) {
                fs::hard_link(first, to).unwrap();
                return;
            } else {
                fs::copy(from, to).unwrap();
                owner_mode(to, meta.uid(), meta.gid(), meta.mode() & 0o7777);
                links.insert(meta.ino(), to.to_path_buf());
            }
            set_mtime(to, (meta.mtime(), meta.mtime_nsec()));
        }
        let mut dirs = Vec::new();
        walk(from, to, &mut HashMap::new(), &mut dirs);
        for (path, meta) in dirs {
            owner_mode(&path, meta.uid(), meta.gid(), meta.mode() & 0o7777);
            set_mtime(&path, (meta.mtime(), meta.mtime_nsec()));
        }
    }

    /// The booted tree's epoch: changes for any write to it, not for reads.
    fn tree_epoch(root: &Path) -> u64 {
        fn walk(path: &Path, rel: &[u8], hash: &mut DefaultHasher) {
            let meta = fs::symlink_metadata(path).unwrap();
            hash.write(rel);
            for value in [
                meta.mode() as u64,
                meta.uid() as u64,
                meta.gid() as u64,
                meta.size(),
                meta.nlink(),
                meta.ino(),
                meta.mtime() as u64,
                meta.mtime_nsec() as u64,
            ] {
                hash.write_u64(value);
            }
            if meta.is_dir() {
                let mut names: Vec<_> = fs::read_dir(path)
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                names.sort();
                for name in names {
                    walk(
                        &path.join(&name),
                        &[rel, b"/", name.as_bytes()].concat(),
                        hash,
                    );
                }
            } else if meta.is_file() {
                let mut bytes = Vec::new();
                fs::File::open(path)
                    .unwrap()
                    .read_to_end(&mut bytes)
                    .unwrap();
                hash.write(&bytes);
            } else {
                hash.write(fs::read_link(path).unwrap().as_os_str().as_bytes());
            }
        }
        let mut hash = DefaultHasher::new();
        walk(root, b"", &mut hash);
        hash.finish()
    }

    /// The BreenFS writer of one boot, over directory roots in `volume/`.
    struct Model {
        volume: PathBuf,
        roots: RefCell<Roots>,
        next: Cell<u64>,
        booted: String,
        attached: RefCell<Option<String>>,
        frozen: Cell<bool>,
        events: RefCell<Vec<String>>,
    }

    impl Model {
        fn path(&self, name: &str) -> PathBuf {
            self.volume.join(name)
        }

        fn id(&self, name: &str) -> Result<(u64, u32), String> {
            self.roots
                .borrow()
                .get(name)
                .copied()
                .ok_or("ENOENT".into())
        }

        fn custody(&self, name: &str) -> Result<(), String> {
            let (number, _) = self.id(name)?;
            if number == 1 || name == self.booted || self.attached.borrow().as_deref() == Some(name)
            {
                return Err("EBUSY".into());
            }
            Ok(())
        }
    }

    impl Writer for Model {
        fn roots(&self) -> Result<Roots, String> {
            Ok(self.roots.borrow().clone())
        }

        fn create(&self, name: &str) -> Result<(), String> {
            if self.roots.borrow().contains_key(name) {
                return Err("EEXIST".into());
            }
            clone_tree(&self.path(&self.booted), &self.path(name));
            let number = self.next.get();
            self.next.set(number + 1);
            self.roots.borrow_mut().insert(name.into(), (number, 0));
            self.events.borrow_mut().push(format!("create {name}"));
            Ok(())
        }

        fn delete(&self, name: &str) -> Result<(), String> {
            self.custody(name)?;
            fs::remove_dir_all(self.path(name)).unwrap();
            self.roots.borrow_mut().remove(name);
            self.events.borrow_mut().push(format!("delete {name}"));
            Ok(())
        }

        fn rename(&self, from: &str, to: &str) -> Result<(), String> {
            self.custody(from)?;
            if self.roots.borrow().contains_key(to) {
                return Err("EEXIST".into());
            }
            fs::rename(self.path(from), self.path(to)).unwrap();
            let entry = self.roots.borrow_mut().remove(from).unwrap();
            self.roots.borrow_mut().insert(to.into(), entry);
            self.events.borrow_mut().push(format!("rename {from} {to}"));
            Ok(())
        }

        fn epoch(&self) -> Result<u64, String> {
            Ok(tree_epoch(&self.path(&self.booted)))
        }

        fn freeze(&self) -> Result<u64, String> {
            self.frozen.set(true);
            self.events.borrow_mut().push("freeze".into());
            self.epoch()
        }

        fn thaw(&self) -> Result<(), String> {
            self.frozen.set(false);
            self.events.borrow_mut().push("thaw".into());
            Ok(())
        }

        fn attach(&self, name: &str) -> Result<(), String> {
            let (number, flags) = self.id(name)?;
            if self.attached.borrow().is_some() || number == 1 || name == self.booted {
                return Err("EBUSY".into());
            }
            if flags != 0 {
                return Err("EROFS".into());
            }
            *self.attached.borrow_mut() = Some(name.into());
            Ok(())
        }

        fn detach(&self) -> Result<(), String> {
            self.attached
                .borrow_mut()
                .take()
                .map(drop)
                .ok_or("EINVAL".into())
        }

        fn stage(&self) -> Result<PathBuf, String> {
            match &*self.attached.borrow() {
                Some(name) => Ok(self.path(name)),
                None => Err("no staging root is mounted".into()),
            }
        }

        fn available(&self) -> Result<u64, String> {
            Ok(64 << 30)
        }
    }

    /// One machine: the root volume, the default tree's shared directories and `/run`.
    struct Machine {
        base: PathBuf,
        stage: PathBuf,
        inventory: Vec<u8>,
        rows: Vec<Row>,
        tool: PathBuf,
        boots: u32,
    }

    impl Drop for Machine {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                fs::remove_dir_all(&self.base).unwrap();
            }
        }
    }

    impl Machine {
        fn home(&self) -> PathBuf {
            self.base.join("default/root")
        }

        fn opt(&self) -> PathBuf {
            self.base.join("default/opt")
        }

        fn boot(&mut self, roots: &Roots, booted: &str) -> Model {
            // /run is emptied at boot.
            let run = self.base.join("run");
            if run.exists() {
                fs::remove_dir_all(&run).unwrap();
            }
            fs::create_dir_all(run.join("breenix")).unwrap();
            self.boots += 1;
            fs::write(
                run.join("breenix/boot-id"),
                format!("{:x}-0-1\n", self.boots),
            )
            .unwrap();
            Model {
                volume: self.base.join("volume"),
                roots: RefCell::new(roots.clone()),
                next: Cell::new(roots.values().map(|(id, _)| id + 1).max().unwrap()),
                booted: booted.into(),
                attached: RefCell::new(None),
                frozen: Cell::new(false),
                events: RefCell::new(Vec::new()),
            }
        }

        fn paired(&self, model: &Model, args: &[&str]) -> Result<String, String> {
            if args == ["protocol"] {
                return Ok(PROTOCOL.into());
            }
            let root = model.path(&model.booted);
            let (home, var, opt) = (self.home(), self.base.join("default/var"), self.opt());
            let run = self.base.join("run");
            let places = Places {
                root: &root,
                run: &run,
                home: &home,
                var: &var,
                opt: &opt,
            };
            let session = Session::open(&places, model.booted.clone(), self.tool.clone(), model)?;
            let mut out = Vec::new();
            let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
            session
                .run(&args, &mut out)
                .map_err(|err| format!("paired {}: {err}", args[0]))?;
            Ok(String::from_utf8(out).unwrap().trim().to_owned())
        }

        /// The host's staging, metal_live_flash.stage_pair, step for step.
        fn deploy(
            &self,
            model: &Model,
            id: &str,
            retained: &[&str],
            upload: impl FnOnce(&Path),
        ) -> Result<serde_json::Value, String> {
            self.deploy_with(model, id, retained, upload, || ())
        }

        fn deploy_with(
            &self,
            model: &Model,
            id: &str,
            retained: &[&str],
            upload: impl FnOnce(&Path),
            before_publish: impl FnOnce(),
        ) -> Result<serde_json::Value, String> {
            let deploy = self.home().join(".breenix-flash").join(id);
            fs::create_dir(&deploy).unwrap();
            fs::set_permissions(&deploy, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(deploy.join("inventory.tsv"), &self.inventory).unwrap();
            assert_eq!(self.paired(model, &["protocol"])?, "BREENIX-PAIRED 1");
            let mut roots: Vec<String> = retained.iter().map(|name| name.to_string()).collect();
            roots.push(model.booted.clone());
            roots.sort();
            roots.dedup();
            let roots: Vec<&str> = roots.iter().map(String::as_str).collect();
            self.paired(model, &[&["reclaim"], &roots[..]].concat())?;
            self.paired(model, &[&["begin", id], &roots[..]].concat())?;
            let have = self.paired(model, &["have", id])?;
            let objects = model.stage().unwrap().join(".breenix-incoming/objects");
            helper(
                &[
                    OsStr::new("objects"),
                    self.stage.as_os_str(),
                    objects.as_os_str(),
                ],
                Some(have.as_bytes()),
            );
            upload(&objects);
            let applied = self.paired(model, &["apply", id])?;
            let receipt: serde_json::Value =
                serde_json::from_str(applied.lines().last().unwrap()).unwrap();
            let keys: BTreeSet<_> = receipt
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, BTreeSet::from(["carried", "deletedOnBox", "onBox"]));
            self.check_verify(&self.paired(model, &["verify", id])?, id);
            before_publish();
            self.paired(model, &["publish", id])?;
            Ok(receipt)
        }

        fn check_verify(&self, line: &str, id: &str) {
            assert_eq!(
                line,
                format!(
                    "paired verify {id} rows={} sha256={}",
                    self.rows.len(),
                    digest(&self.inventory)
                )
            );
        }

        fn have_rows(&self, model: &Model, id: &str) -> BTreeSet<Vec<u8>> {
            self.paired(model, &["have", id])
                .unwrap()
                .lines()
                .map(unhex)
                .collect()
        }

        /// The installer-independent readback of each inventory row, by std::fs.
        fn check_root(&self, root: &Path, applied: &BTreeSet<Vec<u8>>) {
            let mut inodes: HashMap<&[u8], u64> = HashMap::new();
            let mut counts: HashMap<&[u8], u64> = HashMap::new();
            for row in &self.rows {
                if let Some(first) = &row.group {
                    *counts.entry(first.as_slice()).or_default() += 1;
                }
            }
            for row in &self.rows {
                let path = root.join(rel(&row.path));
                let meta =
                    fs::symlink_metadata(&path).unwrap_or_else(|err| panic!("{path:?}: {err}"));
                assert_eq!((meta.uid(), meta.gid()), (row.uid, row.gid), "{path:?}");
                match row.kind.as_str() {
                    "dir" => {
                        assert!(meta.is_dir(), "{path:?}");
                        assert_eq!(meta.mode() & 0o7777, row.mode, "{path:?}");
                    }
                    "reg" => {
                        assert!(meta.is_file(), "{path:?}");
                        assert_eq!(meta.mode() & 0o7777, row.mode, "{path:?}");
                        assert_eq!(digest(&fs::read(&path).unwrap()), row.sha, "{path:?}");
                        let first = row.group.as_deref().unwrap_or(&row.path);
                        let expected = counts.get(first).copied().unwrap_or(1);
                        assert_eq!(meta.nlink(), expected, "{path:?}");
                        assert_eq!(
                            *inodes.entry(first).or_insert(meta.ino()),
                            meta.ino(),
                            "{path:?}"
                        );
                        if applied.contains(&row.path) {
                            assert_eq!((meta.mtime(), meta.mtime_nsec()), row.mtime, "{path:?}");
                        }
                    }
                    _ => {
                        assert!(meta.file_type().is_symlink(), "{path:?}");
                        assert_eq!(
                            digest(fs::read_link(&path).unwrap().as_os_str().as_bytes()),
                            row.sha,
                            "{path:?}"
                        );
                    }
                }
            }
        }
    }

    fn toolchain_manifest(opt: &Path, id: &str, libc: &[u8], loader: &[u8]) {
        let dir = opt.join("breenix/rust").join(id);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/rustc"), b"rustc").unwrap();
        fs::set_permissions(dir.join("bin/rustc"), fs::Permissions::from_mode(0o755)).unwrap();
        let facts = |bytes: &[u8]| serde_json::json!({"size": bytes.len(), "mode": "0755", "sha256": digest(bytes)});
        let manifest = serde_json::json!({
            "id": id, "fork_commit": "0".repeat(40), "files": {"bin/rustc": facts(b"rustc")},
            "runtime": {"libc.so": facts(libc), "ld-breenix.so": facts(loader)},
        });
        fs::write(dir.join("MANIFEST.json"), manifest.to_string()).unwrap();
    }

    /// The first path of `path`'s link group, the one the older bytes were written through.
    fn groups_first<'a>(rows: &'a [Row], path: &'a [u8]) -> &'a [u8] {
        rows.iter()
            .find(|row| row.path == path)
            .and_then(|row| row.group.as_deref())
            .unwrap_or(path)
    }

    fn rehearse(image: &Path, with_baseline: bool) {
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "the rehearsal reproduces root-owned and set-ID files; run it as root (linux.sh)"
        );
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "paired-image-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let stage = base.join("host-stage");
        fs::create_dir_all(&base).unwrap();
        helper(
            &[OsStr::new("inputs"), image.as_os_str(), stage.as_os_str()],
            None,
        );
        let inventory = fs::read(stage.join("inventory.tsv")).unwrap();
        let tools = base
            .join("default/root/.breenix-flash/tools")
            .join("ab".repeat(32));
        let mut m = Machine {
            stage,
            rows: rows(&inventory),
            inventory,
            tool: tools.join("persistent-install"),
            base,
            boots: 0,
        };
        let by_path: BTreeMap<Vec<u8>, Row> = m
            .rows
            .iter()
            .map(|row| (row.path.clone(), row.clone()))
            .collect();

        // The default tree: /root with the staged tool, /var with no pending keys, and /opt
        // with today's devbox selection as a regular file.
        for dir in [
            "default/root",
            "default/var/lib/accounts/pending",
            "default/opt/breenix/rust",
            "volume",
        ] {
            fs::create_dir_all(m.base.join(dir)).unwrap();
        }
        fs::create_dir_all(&tools).unwrap();
        for dir in [
            m.home().join(".breenix-flash"),
            m.home().join(".breenix-flash/tools"),
            tools.clone(),
        ] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(&m.tool, b"staged installer").unwrap();
        fs::set_permissions(&m.tool, fs::Permissions::from_mode(0o700)).unwrap();
        let stale = m.home().join(".breenix-flash/tools").join("cd".repeat(32));
        fs::create_dir(&stale).unwrap();
        fs::write(
            m.opt().join("breenix/rust/CURRENT"),
            format!("{TOOLCHAIN}\n"),
        )
        .unwrap();

        // The running generation: the image as installed, then made an older one. Six of
        // each seven regular inodes hold older bytes (so the deploy replaces them), the
        // largest link group keeps its bytes but loses one /sbin member to a copy, and the
        // devbox runtime sits in /lib beside the image's payload, as on the box today.
        let all = m.base.join("all-objects");
        fs::create_dir(&all).unwrap();
        helper(
            &[OsStr::new("objects"), m.stage.as_os_str(), all.as_os_str()],
            Some(b""),
        );
        let old = m.base.join("volume").join(format!("gen-{OLD}"));
        fs::create_dir(&old).unwrap();
        for mount in [
            "home", "root", "etc", "var", "opt", "srv", "tmp", "run", "dev", "proc", "sys",
        ] {
            fs::create_dir(old.join(mount)).unwrap();
        }
        materialize(&old, &m.rows, &all);
        fs::remove_dir_all(&all).unwrap();
        // An E6 image carries its runtime in /lib and selects a toolchain built against it.
        // A legacy image has neither; the devbox flow put the runtime in /lib after the
        // install recorded its baseline, so the baseline does not list it.
        let devbox = !(by_path.contains_key(&b"/lib/libc.so"[..])
            && by_path.contains_key(&b"/lib/ld-breenix.so"[..]));
        let (libc_bytes, loader_bytes) = if devbox {
            (b"devbox libc".to_vec(), b"devbox loader".to_vec())
        } else {
            (
                fs::read(old.join("lib/libc.so")).unwrap(),
                fs::read(old.join("lib/ld-breenix.so")).unwrap(),
            )
        };
        let selected = by_path.get(&b"/usr/lib/breenix/rust-current"[..]).map(|_| {
            String::from_utf8(fs::read(old.join("usr/lib/breenix/rust-current")).unwrap())
                .unwrap()
                .trim()
                .to_owned()
        });
        let mut groups: HashMap<&[u8], Vec<&Row>> = HashMap::new();
        for row in m.rows.iter().filter(|row| row.kind == "reg") {
            groups
                .entry(row.group.as_deref().unwrap_or(&row.path))
                .or_default()
                .push(row);
        }
        let split = groups.values().max_by_key(|members| members.len()).unwrap()[0]
            .path
            .clone();
        let mut firsts: Vec<_> = groups.keys().copied().collect();
        firsts.sort();
        let mut kept = BTreeSet::new();
        for (index, first) in firsts.iter().enumerate() {
            if index % 7 == 0 || *first == split.as_slice() {
                kept.insert(first.to_vec());
                continue;
            }
            let path = old.join(rel(first));
            let meta = fs::metadata(&path).unwrap();
            fs::write(&path, [b"older:".as_slice(), first].concat()).unwrap();
            owner_mode(&path, meta.uid(), meta.gid(), meta.mode() & 0o7777);
        }
        let split_member = groups[split.as_slice()]
            .iter()
            .find(|row| row.path.starts_with(b"/sbin/") && row.path != split)
            .or_else(|| {
                groups[split.as_slice()]
                    .iter()
                    .find(|row| row.path != split)
            })
            .map(|row| row.path.clone());
        if let Some(member) = &split_member {
            let path = old.join(rel(member));
            let bytes = fs::read(&path).unwrap();
            let meta = fs::metadata(&path).unwrap();
            fs::remove_file(&path).unwrap();
            fs::write(&path, bytes).unwrap();
            owner_mode(&path, meta.uid(), meta.gid(), meta.mode() & 0o7777);
        }
        if let Some(id) = &selected {
            // An E6 image selects its own toolchain, built against its own /lib.
            toolchain_manifest(&m.opt(), id, &libc_bytes, &loader_bytes);
            fs::remove_file(old.join("usr/lib/breenix/rust-current")).unwrap();
        }
        toolchain_manifest(&m.opt(), TOOLCHAIN, &libc_bytes, &loader_bytes);
        fs::write(
            old.join("usr/share/paired-dropped"),
            b"the image dropped this",
        )
        .unwrap();
        if with_baseline {
            let baseline = super::installer::paired::record_fixture(&old).unwrap();
            fs::write(old.join(".breenix-image"), baseline).unwrap();
            fs::set_permissions(
                old.join(".breenix-image"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        fs::write(
            old.join(".complete"),
            format!("BREENIX-USERLAND 2\n{OLD}\nshared etc var opt srv tmp run\n"),
        )
        .unwrap();
        if devbox {
            fs::create_dir_all(old.join("lib")).unwrap();
            for (name, bytes) in [
                ("lib/libc.so", &libc_bytes),
                ("lib/ld-breenix.so", &loader_bytes),
            ] {
                fs::write(old.join(name), bytes).unwrap();
                fs::set_permissions(old.join(name), fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        // Pet changes after that baseline.
        let su = m
            .rows
            .iter()
            .find(|row| row.mode & 0o4000 != 0 && row.group.is_none())
            .expect("a set-ID row")
            .path
            .clone();
        fs::write(old.join(rel(&su)), b"pet su").unwrap();
        fs::set_permissions(old.join(rel(&su)), fs::Permissions::from_mode(0o4755)).unwrap();
        let deleted = m
            .rows
            .iter()
            .rev()
            .find(|row| row.kind == "reg" && row.group.is_none() && row.path != su)
            .unwrap()
            .path
            .clone();
        fs::remove_file(old.join(rel(&deleted))).unwrap();
        fs::create_dir_all(old.join("usr/local/pet")).unwrap();
        fs::write(old.join("usr/local/pet/note"), b"pet").unwrap();
        // PREVIOUS predates the selection file.
        let previous = m.base.join("volume").join(format!("gen-{PREVIOUS}"));
        fs::create_dir_all(previous.join("usr/lib")).unwrap();
        fs::write(previous.join("usr/lib/old-only"), b"previous").unwrap();
        let mut roots = Roots::from([
            ("default".into(), (1, 0)),
            (format!("gen-{PREVIOUS}"), (3, 0)),
            (format!("gen-{OLD}"), (4, 0)),
            // Debris of an earlier attempt, which reclaim removes.
            (format!("{DEBRIS}.new"), (5, 0)),
        ]);
        clone_tree(
            &previous,
            &m.base.join("volume").join(format!("{DEBRIS}.new")),
        );

        // Deploy 1: the image over the older generation.
        let model = m.boot(&roots, &format!("gen-{OLD}"));
        let receipt = m
            .deploy(&model, ID1, &[&format!("gen-{PREVIOUS}")], |_| ())
            .unwrap();
        assert!(
            model.frozen.get(),
            "publish leaves the running root frozen for the reboot"
        );
        assert!(!model.roots.borrow().contains_key(&format!("{DEBRIS}.new")));
        assert!(!stale.exists() && m.tool.exists());
        let paths = |key: &str| -> BTreeSet<String> {
            receipt[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect()
        };
        let display = |path: &[u8]| String::from_utf8_lossy(path).into_owned();
        let gen1 = model.path(&format!("gen-{ID1}"));
        let archive = gen1.join(format!(".breenix-carried/{OLD}"));
        let replaced: BTreeSet<Vec<u8>> = m
            .rows
            .iter()
            .filter(|row| {
                row.kind == "reg" && !kept.contains(row.group.as_deref().unwrap_or(&row.path))
            })
            .map(|row| row.path.clone())
            .collect();
        eprintln!(
            "deploy 1 ({} baseline): {} rows replaced, receipt carried {} deletedOnBox {} onBox {}",
            if with_baseline { "with" } else { "no" },
            replaced.len(),
            paths("carried").len(),
            paths("deletedOnBox").len(),
            paths("onBox").len()
        );
        // Begin's conversion writes the running selection into the running root, a pet
        // change (design 4.5). An E6 inventory replaces it, so apply carries it (4.3).
        const SELECTION: &[u8] = b"/usr/lib/breenix/rust-current";
        let conversion: BTreeSet<String> = selected.iter().map(|_| display(SELECTION)).collect();
        if with_baseline {
            let expected: BTreeSet<String> =
                conversion.iter().cloned().chain([display(&su)]).collect();
            assert_eq!(paths("carried"), expected, "{receipt}");
            assert_eq!(
                paths("deletedOnBox"),
                BTreeSet::from([display(&deleted)]),
                "{receipt}"
            );
            assert!(!gen1.join("usr/share/paired-dropped").exists());
        } else {
            // With no baseline, a payload path that differs from the inventory is an on-box
            // change (design 4.3): carried, not lost. With no baseline to compare, deletedOnBox
            // stays empty.
            let expected: BTreeSet<String> = replaced
                .iter()
                .filter(|path| **path != deleted)
                .map(|path| display(path))
                .chain([display(&su)])
                .chain(conversion.iter().cloned())
                .collect();
            assert_eq!(paths("carried"), expected, "{receipt}");
            assert_eq!(paths("deletedOnBox"), BTreeSet::new(), "{receipt}");
            assert_eq!(
                fs::read(gen1.join("usr/share/paired-dropped")).unwrap(),
                b"the image dropped this"
            );
            assert!(
                paths("onBox").contains("/usr/share/paired-dropped"),
                "{receipt}"
            );
            for path in replaced.iter().filter(|path| **path != deleted) {
                let saved = archive.join(rel(path));
                let meta =
                    fs::symlink_metadata(&saved).unwrap_or_else(|err| panic!("{saved:?}: {err}"));
                assert_eq!(meta.mode() & 0o6000, 0, "{saved:?}");
                let bytes = fs::read(&saved).unwrap();
                if path == SELECTION {
                    assert_eq!(bytes, format!("{TOOLCHAIN}\n").as_bytes(), "{saved:?}");
                } else if *path != su {
                    assert_eq!(
                        bytes,
                        [b"older:".as_slice(), groups_first(&m.rows, path)].concat(),
                        "{saved:?}"
                    );
                }
            }
        }
        assert!(paths("onBox").contains("/usr/local/pet/note"), "{receipt}");
        m.check_root(&gen1, &replaced);
        let carried = archive.join(rel(&su));
        assert_eq!(fs::read(&carried).unwrap(), b"pet su");
        assert_eq!(fs::metadata(&carried).unwrap().mode() & 0o6000, 0);
        assert_eq!(fs::read(gen1.join("usr/local/pet/note")).unwrap(), b"pet");
        assert!(!gen1.join(".breenix-incoming").exists());
        assert_eq!(
            fs::read(gen1.join("usr/share/factory/generation")).unwrap(),
            format!("{ID1}\n").as_bytes()
        );
        for mount in ["home", "root", "etc", "var", "opt"] {
            assert_eq!(fs::read_dir(gen1.join(mount)).unwrap().count(), 0);
        }
        // The toolchain conversion: each retained root names today's selection, and
        // shared CURRENT reads it through the booted generation.
        for root in [&old, &previous] {
            assert_eq!(
                fs::read(root.join("usr/lib/breenix/rust-current")).unwrap(),
                format!("{TOOLCHAIN}\n").as_bytes()
            );
        }
        assert_eq!(
            fs::read_link(m.opt().join("breenix/rust/CURRENT")).unwrap(),
            Path::new("/usr/lib/breenix/rust-current")
        );
        assert_eq!(
            fs::read(old.join(rel(&su))).unwrap(),
            b"pet su",
            "the running root keeps its own files"
        );

        // The trial boots gen-ID1 and runs the cold verify with the same tool.
        roots = model.roots.borrow().clone();
        let model = m.boot(&roots, &format!("gen-{ID1}"));
        m.check_verify(
            &m.paired(&model, &["verify", ID1, "--booted"]).unwrap(),
            ID1,
        );

        // Deploy 2: the same image again. The clone already satisfies the inventory, so no
        // object is uploaded, and apply rereads deploy 1's baseline (its 0xff path included).
        let receipt = m
            .deploy(&model, ID2, &[&format!("gen-{OLD}")], |objects| {
                assert_eq!(
                    fs::read_dir(objects).unwrap().count(),
                    0,
                    "no row needs bytes"
                );
            })
            .unwrap();
        for key in ["carried", "deletedOnBox"] {
            assert_eq!(receipt[key], serde_json::json!([]), "{receipt}");
        }
        assert!(!model
            .roots
            .borrow()
            .contains_key(&format!("gen-{PREVIOUS}")));
        let gen2 = model.path(&format!("gen-{ID2}"));
        m.check_root(&gen2, &BTreeSet::new());
        assert!(!gen2.join(format!(".breenix-carried/{ID1}")).exists());
        roots = model.roots.borrow().clone();
        let model = m.boot(&roots, &format!("gen-{ID2}"));
        m.check_verify(
            &m.paired(&model, &["verify", ID2, "--booted"]).unwrap(),
            ID2,
        );

        // Deploy 3, injected failures. A pet edit makes one row need its object again, and the
        // upload corrupts it: apply refuses, abort removes the draft and its records, and the
        // running root is unchanged and writable.
        let edited = m
            .rows
            .iter()
            .find(|row| row.kind == "reg" && row.group.is_none() && row.size > 0)
            .unwrap();
        fs::write(gen2.join(rel(&edited.path)), b"pet edit").unwrap();
        let before = tree_epoch(&gen2);
        let error = m
            .deploy(&model, ID3, &[&format!("gen-{ID1}")], |objects| {
                let object = objects.join(&edited.sha);
                let mut bytes = fs::read(&object).unwrap();
                bytes[0] ^= 1;
                fs::write(&object, bytes).unwrap();
            })
            .unwrap_err();
        assert!(error.contains("object verification failed"), "{error}");
        assert!(model.roots.borrow().contains_key(&format!("{ID3}.new")));
        m.paired(&model, &["abort", ID3]).unwrap();
        assert!(!model.roots.borrow().contains_key(&format!("{ID3}.new")));
        assert!(model.attached.borrow().is_none() && !model.frozen.get());
        assert!(!m
            .base
            .join(format!("run/breenix/paired/{ID3}.epoch"))
            .exists());
        assert_eq!(tree_epoch(&gen2), before, "the running root is unchanged");

        // The host loses its connection after publish froze the root: abort thaws and keeps
        // the published root, and the next retained set reclaims it.
        m.deploy(&model, ID4, &[&format!("gen-{ID1}")], |_| ())
            .unwrap();
        assert!(model.frozen.get());
        m.paired(&model, &["abort", ID4]).unwrap();
        assert!(!model.frozen.get() && model.roots.borrow().contains_key(&format!("gen-{ID4}")));
        m.paired(&model, &["reclaim", &format!("gen-{ID1}")])
            .unwrap();
        assert!(!model.roots.borrow().contains_key(&format!("gen-{ID4}")));

        // A write to the running root during the deploy: publish refuses, thaws and deletes
        // the draft.
        let error = m
            .deploy_with(
                &model,
                ID5,
                &[&format!("gen-{ID1}")],
                |_| (),
                || {
                    fs::write(
                        gen2.join("usr/local/pet/raced"),
                        b"written during the deploy",
                    )
                    .unwrap();
                },
            )
            .unwrap_err();
        assert!(
            error.contains("running root changed during the deploy"),
            "{error}"
        );
        assert!(!model.roots.borrow().contains_key(&format!("{ID5}.new")));
        assert!(!model.roots.borrow().contains_key(&format!("gen-{ID5}")));
        assert!(!model.frozen.get() && model.attached.borrow().is_none());
        assert_eq!(
            model
                .events
                .borrow()
                .iter()
                .rev()
                .take(3)
                .rev()
                .cloned()
                .collect::<Vec<_>>(),
            ["freeze", "thaw", format!("delete {ID5}.new").as_str()]
        );
    }

    fn fixture(with_baseline: bool) {
        let dir = std::env::temp_dir().join(format!(
            "paired-fixture-{}-{with_baseline}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let image = dir.join("fixture.img");
        helper(&[OsStr::new("fixture"), image.as_os_str()], None);
        rehearse(&image, with_baseline);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn real_image(with_baseline: bool) {
        let Some(image) = std::env::var_os("BREENIX_PAIRED_IMAGE") else {
            eprintln!("skipped: set BREENIX_PAIRED_IMAGE to the ext2.img metal-build delivers");
            return;
        };
        rehearse(Path::new(&image), with_baseline);
    }

    #[test]
    fn rehearses_the_committed_fixture_over_a_recorded_generation() {
        fixture(true);
    }

    #[test]
    fn rehearses_the_committed_fixture_over_a_generation_without_a_baseline() {
        fixture(false);
    }

    #[test]
    fn rehearses_the_real_image_over_a_recorded_generation() {
        real_image(true);
    }

    #[test]
    fn rehearses_the_real_image_over_a_generation_without_a_baseline() {
        real_image(false);
    }
}
