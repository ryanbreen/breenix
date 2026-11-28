//! The live writer's clone, epoch custody and publication protocol.
use super::*;
use crate::{
    breenfs::{is_metal_generation, request},
    breenfs_abi::{
        RootEpoch, SnapshotNext, SnapshotRename, ROOT_EPOCH, ROOT_FREEZE, ROOT_THAW,
        SNAPSHOT_CREATE, SNAPSHOT_DELETE, SNAPSHOT_NEXT, SNAPSHOT_RENAME,
    },
};
use libbreenix::{
    errno::Errno,
    error::Error,
    fs::{fstatfs, mount, umount2},
    syscall::{nr, raw},
    types::Fd,
};
use std::{
    ffi::CString,
    fs,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    thread,
    time::Duration,
};

const STATE: &str = "/run/breenix/paired";
const STAGE: &str = "/run/breenix/paired/stage";
const FLASH: &str = "/root/.breenix-flash";

struct Guest {
    root: Dir,
    runtime: Dir,
    flash: Dir,
    booted: String,
    boot_id: String,
}

fn ctl<T>(file: &File, op: u64, arg: &mut T) -> Result<(), Error> {
    let value =
        unsafe { raw::syscall3(nr::IOCTL, file.as_raw_fd() as u64, op, arg as *mut T as u64) };
    Error::from_syscall(value as i64).map(|_| ())
}

fn mount_list() -> Result<String, String> {
    fs::read_to_string("/proc/mounts").map_err(io_error)
}

fn protected(dir: &Dir, path: &str) -> Result<(), String> {
    let info = dir.file.metadata().map_err(io_error)?;
    if (info.uid(), info.gid(), info.mode() & 0o7777) != (0, 0, 0o700) {
        return Err(format!("{path} must be root:root mode 0700"));
    }
    Ok(())
}

fn valid_root(name: &str) -> bool {
    name.strip_prefix("gen-").is_some_and(crate::valid_id)
}

fn pending_keys() -> Result<(), String> {
    let var = Dir::open(Path::new("/var")).map_err(io_error)?;
    let path = Path::new("lib/accounts/pending");
    let Some((parent, info)) = var.lookup(path).map_err(io_error)? else {
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

fn rust_dir() -> Result<Dir, String> {
    Dir::open(Path::new("/opt"))
        .and_then(|opt| {
            opt.dir(OsStr::new("breenix"), None)?
                .dir(OsStr::new("rust"), None)
        })
        .map_err(io_error)
}

impl Guest {
    fn open() -> Result<Self, String> {
        let root = Dir::open(Path::new("/")).map_err(io_error)?;
        if root.file.metadata().map_err(io_error)?.uid() != 0 {
            return Err("booted root must be owned by root".into());
        }
        let run = Dir::open(Path::new("/run")).map_err(io_error)?;
        let breenix = run.dir(OsStr::new("breenix"), None).map_err(io_error)?;
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
        protected(&runtime, STATE)?;
        let flash = Dir::open(Path::new("/root"))
            .and_then(|root| root.dir(OsStr::new(".breenix-flash"), None))
            .map_err(io_error)?;
        protected(&flash, FLASH)?;
        let mounts = mount_list()?;
        let booted = mounts
            .lines()
            .find_map(|line| {
                let fields: Vec<_> = line.split_ascii_whitespace().collect();
                match fields.as_slice() {
                    [source, "/", "breenfs", flags, ..]
                        if flags.split(',').next() == Some("rw") && valid_root(source) =>
                    {
                        Some(source.to_string())
                    }
                    _ => None,
                }
            })
            .ok_or("paired deploy requires a named writable BreenFS root")?;
        Ok(Self {
            root,
            runtime,
            flash,
            booted,
            boot_id,
        })
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

    fn roots(&self) -> Result<BTreeMap<String, (u64, u32)>, String> {
        let mut next = SnapshotNext {
            id: 0,
            root: request("", 0),
        };
        let mut roots = BTreeMap::new();
        loop {
            ctl(&self.root.file, SNAPSHOT_NEXT, &mut next).map_err(|err| err.to_string())?;
            if next.id == 0 {
                return Ok(roots);
            }
            let name = std::str::from_utf8(&next.root.name[..next.root.name_len as usize])
                .map_err(|err| err.to_string())?;
            roots.insert(name.into(), (next.id, next.root.flags));
        }
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        ctl(&self.root.file, SNAPSHOT_DELETE, &mut request(name, 0))
            .map_err(|err| format!("cannot delete {name}: {err}"))
    }

    fn reclaim(&self, retained: &[String]) -> Result<(), String> {
        let roots = self.roots()?;
        if retained.iter().any(|root| !roots.contains_key(root)) {
            return Err("a retained slot's root is missing".into());
        }
        for (name, (number, _)) in &roots {
            if *number != 1
                && name != &self.booted
                && !retained.contains(name)
                && (name.strip_prefix("gen-").is_some_and(is_metal_generation)
                    || name.strip_suffix(".new").is_some_and(is_metal_generation))
            {
                self.delete(name)?;
            }
        }
        let tools = self
            .flash
            .dir(OsStr::new("tools"), None)
            .map_err(io_error)?;
        let executable = std::env::args_os().next().ok_or("missing tool path")?;
        let active_digest = Path::new(&executable).parent().and_then(Path::file_name);
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
        let data = CString::new(format!("root={name}")).map_err(|err| err.to_string())?;
        unsafe { mount("root-volume", STAGE, "breenfs", 0, data.as_ptr().cast()) }
            .map_err(|err| format!("cannot attach {name}: {err}"))
    }

    fn unmount(&self) -> Result<(), String> {
        umount2(STAGE, 0).map_err(|err| format!("cannot detach {STAGE}: {err}"))
    }

    fn stage(&self) -> Result<Dir, String> {
        if !mount_list()?
            .lines()
            .any(|line| line.split_ascii_whitespace().nth(1) == Some(STAGE))
        {
            return Err("no staging root is mounted".into());
        }
        Dir::open(Path::new(STAGE)).map_err(io_error)
    }

    fn convert(
        &self,
        retained: &[String],
        roots: &BTreeMap<String, (u64, u32)>,
    ) -> Result<(), String> {
        convert_selection(&self.root, &rust_dir()?, |id| {
            for name in retained.iter().filter(|name| *name != &self.booted) {
                let (_, flags) = roots
                    .get(name)
                    .ok_or_else(|| format!("retained root {name} is missing"))?;
                if *flags != 0 {
                    return Err(format!("retained root {name} is read-only"));
                }
                self.mount_stage(name)?;
                let result = (|| {
                    let draft = self.stage()?;
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
        let stats = fstatfs(Fd::from_raw(self.root.file.as_raw_fd() as u64))
            .map_err(|err| err.to_string())?;
        // BreenFS f_bavail already excludes its global reserve and live grants.
        if stats.f_bavail.saturating_mul(stats.f_bsize as u64) < bytes {
            return Err(format!(
                "paired inventory needs {bytes} bytes beyond the BreenFS reserve"
            ));
        }
        pending_keys()?;
        if self
            .root
            .info(OsStr::new(INCOMING))
            .map_err(io_error)?
            .is_some()
        {
            return Err(format!("running root already contains {INCOMING}"));
        }
        baseline(&self.root)?;
        let roots = self.roots()?;
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
        let mut value = RootEpoch::default();
        ctl(&self.root.file, ROOT_EPOCH, &mut value).map_err(|err| err.to_string())?;
        let epoch = epoch_record(value.epoch, &self.boot_id, &self.booted);
        let executable = std::env::args_os().next().ok_or("missing tool path")?;
        let path = Path::new(&executable);
        let sha = path
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .filter(|sha| sha.len() == 64 && is_hex(sha))
            .ok_or("expected a content-addressed staged installer")?;
        if path
            != Path::new(FLASH)
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
        ctl(&self.root.file, SNAPSHOT_CREATE, &mut request(&draft, 0))
            .map_err(|err| err.to_string())?;
        self.mount_stage(&draft)?;
        let stage = self.stage()?;
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
            || self.freeze(),
            |action| match action {
                DraftAction::Detach => self.unmount(),
                DraftAction::CheckPending => pending_keys(),
                DraftAction::Rename => {
                    let mut names = SnapshotRename {
                        from: request(&draft, 0),
                        to: request(&next, 0),
                    };
                    ctl(&self.root.file, SNAPSHOT_RENAME, &mut names).map_err(|err| err.to_string())
                }
                DraftAction::Thaw => self.thaw(),
                DraftAction::Delete => self.delete(&draft),
            },
        )
    }

    fn freeze(&self) -> Result<u64, String> {
        let mut value = RootEpoch::default();
        let mut retries = 2;
        loop {
            match ctl(&self.root.file, ROOT_FREEZE, &mut value) {
                Ok(()) => return Ok(value.epoch),
                Err(Error::Os(Errno::EBUSY)) if retries != 0 => {
                    retries -= 1;
                    thread::sleep(Duration::from_secs(1));
                }
                Err(err) => {
                    return Err(format!(
                        "root freeze: {err} (writable mappings {}, dirty files {})",
                        value.writable_mappings, value.dirty_files
                    ))
                }
            }
        }
    }

    fn thaw(&self) -> Result<(), String> {
        ctl(&self.root.file, ROOT_THAW, &mut RootEpoch::default()).map_err(|err| err.to_string())
    }

    fn abort(&self, id: &str) -> Result<(), String> {
        self.epoch(id)?;
        self.thaw()?;
        if self.stage().is_ok() {
            self.unmount()?;
        }
        let draft = format!("{id}.new");
        if self.roots()?.contains_key(&draft) {
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

    fn verify(&self, id: &str, booted: bool) -> Result<(), String> {
        let inventory = self.inventory(id)?;
        let stage;
        let root = if booted {
            if self.booted != format!("gen-{id}") {
                return Err("wrong booted root".into());
            }
            &self.root
        } else {
            self.epoch(id)?;
            stage = self.stage()?;
            &stage
        };
        root.file.sync_all().map_err(io_error)?;
        let sha = inventory.verify(root)?;
        if !booted {
            if root.info(OsStr::new(INCOMING)).map_err(io_error)?.is_some()
                || root.read(OsStr::new(".complete")).map_err(io_error)?
                    != layout::record(id).as_bytes()
                || field(root, GENERATION).map_err(io_error)? != format!("{id}\n").as_bytes()
            {
                return Err("the staging root has not finished apply".into());
            }
            layout::check_setid(Path::new(STAGE), false)?;
        }
        toolchain(root, &rust_dir()?)?;
        if !booted {
            self.runtime
                .write(OsStr::new(&format!("{id}.verified")), sha.as_bytes(), 0o600)
                .map_err(io_error)?;
        }
        println!(
            "paired verify {id} rows={} sha256={sha}",
            inventory.rows.len()
        );
        Ok(())
    }
}

pub(super) fn run(args: Vec<String>) -> Result<(), String> {
    if args == ["protocol"] {
        println!("BREENIX-PAIRED 1");
        return Ok(());
    }
    let Some((verb, args)) = args.split_first() else {
        return Err("missing paired subcommand".into());
    };
    let guest = Guest::open()?;
    if verb == "reclaim" {
        if !args.iter().all(|name| valid_root(name)) {
            return Err("invalid retained root".into());
        }
        return guest.reclaim(args);
    }
    let Some((id, rest)) = args.split_first().filter(|(id, _)| is_metal_generation(id)) else {
        return Err("invalid paired generation ID".into());
    };
    match (verb.as_str(), rest) {
        ("begin", roots) if roots.iter().all(|name| valid_root(name)) => guest.begin(id, roots),
        ("have", []) => {
            guest.epoch(id)?;
            let inventory = guest.inventory(id)?;
            for index in inventory.have(&guest.stage()?)? {
                let row = &inventory.rows[index];
                println!(
                    "{}",
                    local::hex([b"/", row.path.as_os_str().as_bytes()].concat())
                );
            }
            Ok(())
        }
        ("apply", []) => {
            guest.epoch(id)?;
            let stamp = format!("{id}.verified");
            let stamp = OsStr::new(&stamp);
            if guest.runtime.info(stamp).map_err(io_error)?.is_some() {
                guest.runtime.remove(stamp, false).map_err(io_error)?;
            }
            let previous = guest
                .booted
                .strip_prefix("gen-")
                .ok_or("missing previous generation")?;
            let report = apply(
                &guest.stage()?,
                id,
                previous,
                &guest.inventory(id)?,
                &mut || Ok(()),
            )?;
            println!("{report}");
            Ok(())
        }
        ("verify", []) => guest.verify(id, false),
        ("verify", [option]) if option == "--booted" => guest.verify(id, true),
        ("publish", []) => guest.publish(id),
        ("abort", []) => guest.abort(id),
        _ => Err("invalid paired subcommand or arguments".into()),
    }
}
