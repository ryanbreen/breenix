//! The paired subcommands over one writer: epoch custody, the clone, apply,
//! verify and publication. The box drives the BreenFS ioctls (guest.rs); the
//! host rehearsal drives a directory model of the same writer.
use super::*;
use std::{io::Write, os::unix::fs::MetadataExt};

/// Root names and their (number, flags) as `SNAPSHOT_NEXT` lists them.
pub(crate) type Roots = BTreeMap<String, (u64, u32)>;

/// The operations the paired subcommands need from the booted BreenFS writer.
pub(crate) trait Writer {
    fn roots(&self) -> Result<Roots, String>;
    /// `SNAPSHOT_CREATE` of the booted root, flags 0.
    fn create(&self, name: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    fn rename(&self, from: &str, to: &str) -> Result<(), String>;
    fn epoch(&self) -> Result<u64, String>;
    fn freeze(&self) -> Result<u64, String>;
    fn thaw(&self) -> Result<(), String>;
    /// Mount `name` with `root-volume` at the staging path.
    fn attach(&self, name: &str) -> Result<(), String>;
    fn detach(&self) -> Result<(), String>;
    /// The staging path, when a staging root is mounted there.
    fn stage(&self) -> Result<PathBuf, String>;
    /// Free bytes beyond the BreenFS reserve.
    fn available(&self) -> Result<u64, String>;
}

/// Where the session finds the trees it reads and writes.
pub(crate) struct Places<'a> {
    pub root: &'a Path,
    pub run: &'a Path,
    /// `/root`, the default tree's directory.
    pub home: &'a Path,
    pub var: &'a Path,
    pub opt: &'a Path,
}

pub(crate) struct Session<'w> {
    root: Dir,
    runtime: Dir,
    flash: Dir,
    flash_path: PathBuf,
    var: Dir,
    opt: Dir,
    booted: String,
    boot_id: String,
    tool: PathBuf,
    writer: &'w dyn Writer,
}

fn protected(dir: &Dir, path: &str) -> Result<(), String> {
    let info = dir.file.metadata().map_err(io_error)?;
    if (info.uid(), info.gid(), info.mode() & 0o7777) != (0, 0, 0o700) {
        return Err(format!("{path} must be root:root mode 0700"));
    }
    Ok(())
}

pub(super) fn valid_root(name: &str) -> bool {
    name.strip_prefix("gen-").is_some_and(valid_id)
}

impl<'w> Session<'w> {
    pub(crate) fn open(
        places: &Places,
        booted: String,
        tool: PathBuf,
        writer: &'w dyn Writer,
    ) -> Result<Self, String> {
        let root = Dir::open(places.root).map_err(io_error)?;
        if root.file.metadata().map_err(io_error)?.uid() != 0 {
            return Err("booted root must be owned by root".into());
        }
        let breenix = Dir::open(places.run)
            .and_then(|run| run.dir(OsStr::new("breenix"), None))
            .map_err(io_error)?;
        let boot_id = String::from_utf8(breenix.read(OsStr::new("boot-id")).map_err(io_error)?)
            .map_err(|err| err.to_string())?;
        let boot_id = boot_id.trim().to_owned();
        if boot_id.is_empty()
            || !boot_id
                .bytes()
                .all(|ch| ch.is_ascii_hexdigit() || ch == b'-')
        {
            return Err("invalid running boot ID".into());
        }
        let runtime = breenix
            .dir(OsStr::new("paired"), Some(0o700))
            .map_err(io_error)?;
        protected(&runtime, "/run/breenix/paired")?;
        let flash = Dir::open(places.home)
            .and_then(|root| root.dir(OsStr::new(".breenix-flash"), None))
            .map_err(io_error)?;
        protected(&flash, "/root/.breenix-flash")?;
        if !valid_root(&booted) {
            return Err("paired deploy requires a named writable BreenFS root".into());
        }
        Ok(Self {
            root,
            runtime,
            flash,
            flash_path: places.home.join(".breenix-flash"),
            var: Dir::open(places.var).map_err(io_error)?,
            opt: Dir::open(places.opt).map_err(io_error)?,
            booted,
            boot_id,
            tool,
            writer,
        })
    }

    fn pending_keys(&self) -> Result<(), String> {
        let path = Path::new("lib/accounts/pending");
        let Some((parent, info)) = self.var.lookup(path).map_err(io_error)? else {
            return Ok(());
        };
        if info.kind() == DIR
            && parent
                .dir(OsStr::new("pending"), None)
                .map_err(io_error)?
                .names()
                .map_err(io_error)?
                .is_empty()
        {
            return Ok(());
        }
        Err(
            "pending account keys in /var/lib/accounts/pending must be reconciled before deploying"
                .into(),
        )
    }

    fn rust_dir(&self) -> Result<Dir, String> {
        self.opt
            .dir(OsStr::new("breenix"), None)
            .and_then(|breenix| breenix.dir(OsStr::new("rust"), None))
            .map_err(io_error)
    }

    fn deploy(&self, id: &str) -> Result<Dir, String> {
        let dir = self.flash.dir(OsStr::new(id), None).map_err(io_error)?;
        protected(&dir, id)?;
        Ok(dir)
    }

    fn inventory(&self, id: &str) -> Result<Inventory, String> {
        Inventory::parse(
            &self
                .deploy(id)?
                .read(OsStr::new("inventory.tsv"))
                .map_err(io_error)?,
        )
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.writer
            .delete(name)
            .map_err(|err| format!("cannot delete {name}: {err}"))
    }

    fn reclaim(&self, retained: &[String]) -> Result<(), String> {
        let roots = self.writer.roots()?;
        if retained.iter().any(|root| !roots.contains_key(root)) {
            return Err("a retained slot's root is missing".into());
        }
        for (name, (number, _)) in &roots {
            if reclaimable(name, *number, retained, &self.booted) {
                self.delete(name)?;
            }
        }
        let tools = self
            .flash
            .dir(OsStr::new("tools"), None)
            .map_err(io_error)?;
        let active_digest = self.tool.parent().and_then(Path::file_name);
        let mut keep = BTreeSet::new();
        for root in retained.iter().chain(std::iter::once(&self.booted)) {
            let id = root.strip_prefix("gen-").ok_or("invalid retained root")?;
            if let Some((dir, _)) = self
                .flash
                .lookup(&Path::new(id).join("tool"))
                .map_err(io_error)?
            {
                let data = dir.read(OsStr::new("tool")).map_err(io_error)?;
                let sha = std::str::from_utf8(&data)
                    .map_err(|err| err.to_string())?
                    .trim();
                if sha.len() != 64 || !is_hex(sha) {
                    return Err("invalid retained tool record".into());
                }
                keep.insert(OsString::from(sha));
            }
        }
        for name in tools.names().map_err(io_error)? {
            if name
                .to_str()
                .is_some_and(|name| name.len() == 64 && is_hex(name))
                && Some(name.as_os_str()) != active_digest
                && !keep.contains(&name)
            {
                tools.remove_tree(&name).map_err(io_error)?;
            }
        }
        Ok(())
    }

    fn mount_stage(&self, name: &str) -> Result<(), String> {
        self.runtime
            .dir(OsStr::new("stage"), Some(0o700))
            .map_err(io_error)?;
        self.writer
            .attach(name)
            .map_err(|err| format!("cannot attach {name}: {err}"))
    }

    fn unmount(&self) -> Result<(), String> {
        self.writer
            .detach()
            .map_err(|err| format!("cannot detach /run/breenix/paired/stage: {err}"))
    }

    fn stage(&self) -> Result<(Dir, PathBuf), String> {
        let path = self.writer.stage()?;
        Ok((Dir::open(&path).map_err(io_error)?, path))
    }

    fn convert(&self, retained: &[String], roots: &Roots) -> Result<(), String> {
        convert_selection(&self.root, &self.rust_dir()?, |id| {
            for name in retained.iter().filter(|name| *name != &self.booted) {
                let (_, flags) = roots
                    .get(name)
                    .ok_or_else(|| format!("retained root {name} is missing"))?;
                if *flags != 0 {
                    return Err(format!("retained root {name} is read-only"));
                }
                self.mount_stage(name)?;
                let result = (|| {
                    let (draft, _) = self.stage()?;
                    seed_selection(&draft, id, name, *flags == 0)
                })();
                let detach = self.unmount();
                result.and(detach)?;
            }
            Ok(())
        })
    }

    fn begin(&self, id: &str, retained: &[String]) -> Result<(), String> {
        let inventory = self.inventory(id)?;
        let bytes = inventory
            .rows
            .iter()
            .filter(|row| row.kind == REG)
            .try_fold(0u64, |sum, row| sum.checked_add(row.size))
            .and_then(|sum| sum.checked_mul(2))
            .ok_or("paired inventory is too large")?;
        // BreenFS f_bavail already excludes its global reserve and live grants.
        if self.writer.available()? < bytes {
            return Err(format!(
                "paired inventory needs {bytes} bytes beyond the BreenFS reserve"
            ));
        }
        self.pending_keys()?;
        if self
            .root
            .info(OsStr::new(INCOMING))
            .map_err(io_error)?
            .is_some()
        {
            return Err(format!("running root already contains {INCOMING}"));
        }
        baseline(&self.root)?;
        let roots = self.writer.roots()?;
        let draft = format!("{id}.new");
        if roots.contains_key(&draft)
            || roots.contains_key(&format!("gen-{id}"))
            || self
                .runtime
                .info(OsStr::new(&format!("{id}.epoch")))
                .map_err(io_error)?
                .is_some()
        {
            return Err("this deploy ID already has a root or epoch record".into());
        }
        self.convert(retained, &roots)?;
        let epoch = epoch_record(self.writer.epoch()?, &self.boot_id, &self.booted);
        let sha = self
            .tool
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .filter(|sha| sha.len() == 64 && is_hex(sha))
            .ok_or("expected a content-addressed staged installer")?;
        if self.tool
            != self
                .flash_path
                .join("tools")
                .join(sha)
                .join("persistent-install")
        {
            return Err("expected the staged installer path".into());
        }
        self.deploy(id)?
            .write(OsStr::new("tool"), format!("{sha}\n").as_bytes(), 0o600)
            .map_err(io_error)?;
        self.runtime
            .write(OsStr::new(&format!("{id}.epoch")), epoch.as_bytes(), 0o600)
            .map_err(io_error)?;
        self.writer.create(&draft)?;
        self.mount_stage(&draft)?;
        let (stage, _) = self.stage()?;
        let incoming = stage
            .dir(OsStr::new(INCOMING), Some(0o700))
            .map_err(io_error)?;
        protected(&incoming, INCOMING)?;
        incoming
            .dir(OsStr::new("objects"), Some(0o700))
            .map_err(io_error)?;
        Ok(())
    }

    fn epoch(&self, id: &str) -> Result<u64, String> {
        let bytes = self
            .runtime
            .read(OsStr::new(&format!("{id}.epoch")))
            .map_err(io_error)?;
        read_epoch(&bytes, &self.boot_id, &self.booted)
    }

    fn publish(&self, id: &str) -> Result<(), String> {
        let first = self.epoch(id)?;
        let inventory = self.inventory(id)?;
        let stamp = format!("{id}.verified");
        if self.runtime.read(OsStr::new(&stamp)).map_err(io_error)? != inventory.sha.as_bytes() {
            return Err("the draft has no matching verify receipt".into());
        }
        let draft = format!("{id}.new");
        let next = format!("gen-{id}");
        publish_root(
            first,
            || self.writer.freeze(),
            |action| match action {
                DraftAction::Detach => self.unmount(),
                DraftAction::CheckPending => self.pending_keys(),
                DraftAction::Rename => self.writer.rename(&draft, &next),
                DraftAction::Thaw => self.writer.thaw(),
                DraftAction::Delete => self.delete(&draft),
            },
        )
    }

    fn abort(&self, id: &str) -> Result<(), String> {
        self.epoch(id)?;
        self.writer.thaw()?;
        if self.stage().is_ok() {
            self.unmount()?;
        }
        let draft = format!("{id}.new");
        if self.writer.roots()?.contains_key(&draft) {
            self.delete(&draft)?;
        }
        // A published root is kept until the host supplies the next retained set.
        for suffix in ["epoch", "verified"] {
            let name = format!("{id}.{suffix}");
            let name = OsStr::new(&name);
            if self.runtime.info(name).map_err(io_error)?.is_some() {
                self.runtime.remove(name, false).map_err(io_error)?;
            }
        }
        Ok(())
    }

    fn verify(&self, id: &str, booted: bool, out: &mut dyn Write) -> Result<(), String> {
        let inventory = self.inventory(id)?;
        let stage;
        let (root, path) = if booted {
            if self.booted != format!("gen-{id}") {
                return Err("wrong booted root".into());
            }
            (&self.root, None)
        } else {
            self.epoch(id)?;
            stage = self.stage()?;
            (&stage.0, Some(stage.1.as_path()))
        };
        root.file.sync_all().map_err(io_error)?;
        let sha = inventory.verify(root)?;
        if let Some(path) = path {
            if root.info(OsStr::new(INCOMING)).map_err(io_error)?.is_some()
                || root.read(OsStr::new(".complete")).map_err(io_error)?
                    != layout::record(id).as_bytes()
                || field(root, GENERATION).map_err(io_error)? != format!("{id}\n").as_bytes()
            {
                return Err("the staging root has not finished apply".into());
            }
            layout::check_setid(path, false)?;
        }
        toolchain(root, &self.rust_dir()?)?;
        if !booted {
            self.runtime
                .write(OsStr::new(&format!("{id}.verified")), sha.as_bytes(), 0o600)
                .map_err(io_error)?;
        }
        writeln!(
            out,
            "paired verify {id} rows={} sha256={sha}",
            inventory.rows.len()
        )
        .map_err(io_error)
    }

    /// Run one subcommand after `protocol`, writing what the box prints to `out`.
    pub(crate) fn run(&self, args: &[String], out: &mut dyn Write) -> Result<(), String> {
        let Some((verb, args)) = args.split_first() else {
            return Err("missing paired subcommand".into());
        };
        if verb == "reclaim" {
            if !args.iter().all(|name| valid_root(name)) {
                return Err("invalid retained root".into());
            }
            return self.reclaim(args);
        }
        let Some((id, rest)) = args.split_first().filter(|(id, _)| is_metal_generation(id)) else {
            return Err("invalid paired generation ID".into());
        };
        match (verb.as_str(), rest) {
            ("begin", roots) if roots.iter().all(|name| valid_root(name)) => self.begin(id, roots),
            ("have", []) => {
                self.epoch(id)?;
                let inventory = self.inventory(id)?;
                for index in inventory.have(&self.stage()?.0)? {
                    let row = &inventory.rows[index];
                    writeln!(
                        out,
                        "{}",
                        local::hex([b"/", row.path.as_os_str().as_bytes()].concat())
                    )
                    .map_err(io_error)?;
                }
                Ok(())
            }
            ("apply", []) => {
                self.epoch(id)?;
                let stamp = format!("{id}.verified");
                let stamp = OsStr::new(&stamp);
                if self.runtime.info(stamp).map_err(io_error)?.is_some() {
                    self.runtime.remove(stamp, false).map_err(io_error)?;
                }
                let previous = self
                    .booted
                    .strip_prefix("gen-")
                    .ok_or("missing previous generation")?;
                let report = apply(
                    &self.stage()?.0,
                    &self.rust_dir()?,
                    id,
                    previous,
                    &self.inventory(id)?,
                    &mut || Ok(()),
                )?;
                writeln!(out, "{report}").map_err(io_error)
            }
            ("verify", []) => self.verify(id, false, out),
            ("verify", [option]) if option == "--booted" => self.verify(id, true, out),
            ("publish", []) => self.publish(id),
            ("abort", []) => self.abort(id),
            _ => Err("invalid paired subcommand or arguments".into()),
        }
    }
}

/// The reply to `paired protocol`, which needs no writer.
pub(crate) const PROTOCOL: &str = "BREENIX-PAIRED 1";
