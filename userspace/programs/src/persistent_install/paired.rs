//! Apply a sealed userspace inventory to a writable clone of the booted root.
use super::{is_metal_generation, layout, local, valid_id};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

#[path = "paired/tree.rs"]
mod tree;
use tree::{Dir, Info, DIR, LINK, REG};

#[path = "paired/session.rs"]
pub(crate) mod session;

#[cfg(target_os = "breenix")]
#[path = "paired/guest.rs"]
mod guest;
#[cfg(target_os = "breenix")]
pub(super) fn run(args: Vec<String>) -> Result<(), String> {
    guest::run(args)
}

const INCOMING: &str = ".breenix-incoming";
const SELECTION: &str = "usr/lib/breenix/rust-current";
const GENERATION: &str = "usr/share/factory/generation";
const BASELINE: &str = ".breenix-image";
const MAX_BATCH: u64 = 32 * 1024 * 1024;

fn io_error(err: io::Error) -> String {
    err.to_string()
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    local::hex(local::sha256::sha256(bytes))
}

fn digest_file(mut input: File) -> io::Result<String> {
    let mut hash = local::sha256::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(local::hex(hash.finalize()));
        }
        hash.update(&buffer[..count]);
    }
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !is_hex(text) {
        return Err("invalid inventory hex".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|err| err.to_string()))
        .collect()
}

fn is_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn payload(path: &Path) -> bool {
    matches!(
        path.components()
            .next()
            .and_then(|name| name.as_os_str().to_str()),
        Some("bin" | "sbin" | "lib" | "usr")
    )
}

fn decode_path(hex: &str) -> Result<PathBuf, String> {
    let bytes = unhex(hex)?;
    let path = bytes
        .strip_prefix(b"/")
        .ok_or("inventory path is not absolute")?;
    if path
        .split(|&b| b == b'/')
        .any(|part| matches!(part, b"" | b"." | b"..") || part.contains(&0))
    {
        return Err("invalid inventory path".into());
    }
    let path = PathBuf::from(OsString::from_vec(path.to_vec()));
    if !payload(&path) || path == Path::new(GENERATION) {
        return Err(format!("path outside payload: {path:?}"));
    }
    Ok(path)
}

struct Row {
    path: PathBuf,
    kind: u32,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: (i64, i64),
    digest: String,
}

struct Inventory {
    rows: Vec<Row>,
    groups: Vec<Vec<usize>>,
    sha: String,
}

impl Inventory {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|err| err.to_string())?;
        if text.is_empty() || !text.ends_with('\n') {
            return Err("incomplete paired inventory".into());
        }
        let mut rows: Vec<Row> = Vec::new();
        let mut groups = Vec::<Vec<usize>>::new();
        let mut linked = BTreeMap::<PathBuf, usize>::new();
        let mut dirs = BTreeMap::new();
        for line in text.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            let [path, kind, mode, uid, gid, group, size, mtime, sha] = fields.as_slice() else {
                return Err("invalid inventory row".into());
            };
            let path = decode_path(path)?;
            if rows
                .last()
                .is_some_and(|prev| prev.path.as_os_str().as_bytes() >= path.as_os_str().as_bytes())
            {
                return Err(format!("inventory out of order at {path:?}"));
            }
            if path
                .ancestors()
                .skip(1)
                .any(|parent| dirs.get(parent) == Some(&false))
            {
                return Err(format!("non-directory inventory parent of {path:?}"));
            }
            let kind = match *kind {
                "reg" => REG,
                "dir" => DIR,
                "symlink" => LINK,
                _ => return Err(format!("unsupported inventory type at {path:?}")),
            };
            let mode = u32::from_str_radix(mode, 8)
                .ok()
                .filter(|value| *value <= 0o7777 && format!("{value:04o}") == *mode)
                .ok_or("invalid inventory mode")?;
            let uid = uid.parse().map_err(|_| "invalid inventory uid")?;
            let gid = gid.parse().map_err(|_| "invalid inventory gid")?;
            let size = size.parse().map_err(|_| "invalid inventory size")?;
            let (sec, ns) = mtime.split_once('.').ok_or("invalid inventory mtime")?;
            let mut time = (
                sec.parse().map_err(|_| "invalid inventory seconds")?,
                ns.parse().map_err(|_| "invalid inventory nanoseconds")?,
            );
            if ns.len() != 9 || !(0..1_000_000_000).contains(&time.1) {
                return Err("invalid inventory nanoseconds".into());
            }
            if kind == DIR {
                if size != 0 || *sha != "-" || *group != "-" {
                    return Err("invalid directory inventory row".into());
                }
            } else if sha.len() != 64
                || !is_hex(sha)
                || kind == LINK && (mode != 0o777 || *group != "-" || size > 4095)
            {
                return Err("invalid file inventory row".into());
            }
            if kind == LINK && size == 0 {
                return Err("empty symlink target".into());
            }
            let index = rows.len();
            if *group == "-" {
                groups.push(vec![index]);
            } else {
                let key = decode_path(group)?;
                if key == path {
                    linked.insert(key, groups.len());
                    groups.push(vec![index]);
                } else {
                    let slot = linked
                        .get(&key)
                        .ok_or("missing first path of inventory link group")?;
                    let first = &rows[groups[*slot][0]];
                    if (
                        first.kind,
                        first.mode,
                        first.uid,
                        first.gid,
                        first.size,
                        first.digest.as_str(),
                        first.mtime,
                    ) != (kind, mode, uid, gid, size, *sha, time)
                    {
                        return Err("inventory link group has inconsistent metadata".into());
                    }
                    groups[*slot].push(index);
                    time = first.mtime;
                }
            }
            dirs.insert(path.clone(), kind == DIR);
            rows.push(Row {
                path,
                kind,
                mode,
                uid,
                gid,
                size,
                mtime: time,
                digest: (*sha).into(),
            });
        }
        if linked.values().any(|&index| groups[index].len() < 2) {
            return Err("singleton inventory link group".into());
        }
        Ok(Self {
            rows,
            groups,
            sha: digest(bytes),
        })
    }

    fn matches(&self, root: &Dir) -> Result<Vec<bool>, String> {
        let mut valid = vec![false; self.groups.len()];
        for (index, group) in self.groups.iter().enumerate() {
            let mut inode = None;
            let mut matched = true;
            for &member in group {
                let row = &self.rows[member];
                let Some((dir, info)) = root.lookup(&row.path).map_err(io_error)? else {
                    matched = false;
                    continue;
                };
                if !row.matches(&dir, info).map_err(io_error)? {
                    matched = false;
                }
                if info.kind() == REG
                    && (info.links != group.len() as u64
                        || *inode.get_or_insert(info.ino) != info.ino)
                {
                    matched = false;
                }
            }
            valid[index] = matched;
        }
        Ok(valid)
    }

    fn have(&self, root: &Dir) -> Result<Vec<usize>, String> {
        let matches = self.matches(root)?;
        Ok(self
            .groups
            .iter()
            .zip(matches)
            .filter(|(_, yes)| *yes)
            .flat_map(|(group, _)| group.iter().copied())
            .collect())
    }

    fn verify(&self, root: &Dir) -> Result<String, String> {
        for (group, yes) in self.groups.iter().zip(self.matches(root)?) {
            if !yes {
                return Err(format!(
                    "inventory mismatch at {:?}",
                    self.rows[group[0]].path
                ));
            }
        }
        // Reuse the publisher's mtime column after comparing the material
        // fields. An unchanged file can keep its on-box mtime.
        Ok(self.sha.clone())
    }
}

impl Row {
    fn leaf(&self) -> &OsStr {
        self.path.file_name().expect("validated inventory path")
    }

    fn matches(&self, dir: &Dir, info: Info) -> io::Result<bool> {
        if info.kind() != self.kind
            || (info.uid, info.gid) != (self.uid, self.gid)
            || self.kind != LINK && info.mode & 0o7777 != self.mode
            || self.kind != DIR && info.size != self.size
        {
            return Ok(false);
        }
        Ok(match self.kind {
            REG => digest_file(dir.read_file(self.leaf())?)? == self.digest,
            LINK => digest(&dir.read_link(self.leaf())?) == self.digest,
            _ => true,
        })
    }
}

fn fingerprint(dir: &Dir, leaf: &OsStr, info: Info) -> io::Result<String> {
    let (mode, uid, gid) = (info.mode, info.uid, info.gid);
    match info.kind() {
        DIR => Ok("directory".into()),
        REG => local::identity(mode, uid, gid, dir.read_file(leaf)?),
        LINK => local::identity(mode, uid, gid, &dir.read_link(leaf)?[..]),
        _ => Ok("unsupported".into()),
    }
}

fn baseline(root: &Dir) -> Result<Option<BTreeMap<PathBuf, String>>, String> {
    let name = OsStr::new(BASELINE);
    if root.info(name).map_err(io_error)?.is_none() {
        return Ok(None);
    }
    let bytes = root.read(name).map_err(io_error)?;
    local::parse_baseline(&bytes).map(Some)
}

fn list_payload(root: &Dir, path: &Path, out: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    let Some((dir, info)) = root.lookup(path)? else {
        return Ok(());
    };
    out.insert(path.to_path_buf());
    if info.kind() == DIR {
        let child = dir.dir(path.file_name().unwrap(), None)?;
        for name in child.names()? {
            list_payload(root, &path.join(name), out)?;
        }
    }
    Ok(())
}

fn display_path(path: &Path) -> String {
    format!("/{}", path.to_string_lossy())
}

fn write_path(root: &Dir, path: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
    let (dir, leaf) = root.parent(Path::new(path), true)?;
    if dir.info(leaf)?.is_some_and(|info| info.kind() != REG) {
        return Err(io::Error::other(format!("{path}: expected a regular file")));
    }
    dir.write(leaf, bytes, mode)
}

fn set_selection(root: &Dir, id: &str) -> io::Result<()> {
    write_path(root, SELECTION, format!("{id}\n").as_bytes(), 0o644)
}

fn field(root: &Dir, path: &str) -> io::Result<Vec<u8>> {
    let (dir, leaf) = root.parent(Path::new(path), false)?;
    dir.read(leaf)
}

fn object(objects: &Dir, row: &Row) -> io::Result<File> {
    let name = OsStr::new(&row.digest);
    let info = objects
        .info(name)?
        .ok_or_else(|| io::Error::other(format!("missing object {name:?}")))?;
    let file = objects.read_file(name)?;
    if info.mode & 0o7777 != 0o600
        || info.links != 1
        || info.size != row.size
        || digest_file(file.try_clone()?)? != row.digest
    {
        return Err(io::Error::other(format!(
            "object verification failed for {:?}",
            row.path
        )));
    }
    use std::io::{Seek, SeekFrom};
    let mut file = file;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

fn carry(root: &Dir, path: &Path, previous: &str, info: Info) -> io::Result<()> {
    let target = Path::new(".breenix-carried").join(previous).join(path);
    let (old, leaf) = root.parent(path, false)?;
    let archive = root.dir(OsStr::new(".breenix-carried"), Some(0o700))?;
    archive
        .file
        .set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    let (new, name) = root.parent(&target, true)?;
    let archived = new.info(name)?;
    if info.mode & 0o6000 != 0 && info.kind() == REG {
        let file = old.read_file(leaf)?;
        if info.links > 1 {
            // Clearing permissions on the old inode would also modify
            // unrelated pet links. Publish an independent archive first.
            let mode = info.mode & 0o1777;
            if let Some(saved) = archived {
                if saved.kind() != REG
                    || saved.mode & 0o7777 != mode
                    || (saved.uid, saved.gid, saved.size) != (info.uid, info.gid, info.size)
                    || digest_file(new.read_file(name)?)? != digest_file(file)?
                {
                    return Err(io::Error::other(format!(
                        "carry destination differs: {target:?}"
                    )));
                }
            } else {
                new.atomic(name, |temp| {
                    let mut input = file;
                    let mut output = new.create(temp)?;
                    io::copy(&mut input, &mut output)?;
                    tree::owner_mode(&output, info.uid, info.gid, mode)?;
                    output.sync_all()
                })?;
            }
            old.remove(leaf, false)?;
            return old.file.sync_all();
        }
        if archived.is_none() {
            // Clear before moving the last link so an interrupted rename
            // cannot strand a privileged file in the archive.
            tree::owner_mode(&file, info.uid, info.gid, info.mode & 0o1777)?;
        }
    }
    if archived.is_some() {
        return Err(io::Error::other(format!(
            "carry destination already exists: {target:?}"
        )));
    }
    old.rename(leaf, &new, name)?;
    new.file.sync_all()
}

fn checked_retirement(path: &Path) -> bool {
    payload(path) && path != Path::new(SELECTION) && path != Path::new(GENERATION)
}

fn apply(
    root: &Dir,
    rust: &Dir,
    id: &str,
    previous: &str,
    inventory: &Inventory,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<String, String> {
    let old = baseline(root)?.unwrap_or_default();
    let valid = inventory.matches(root)?;
    let (toolchain, runtime) = runtime_plan(root, rust, inventory)?;
    // Runtime files take the draft root's owner, root:root on the box.
    let owner = {
        use std::os::unix::fs::MetadataExt as _;
        let meta = root.file.metadata().map_err(io_error)?;
        (meta.uid(), meta.gid())
    };
    let incoming_name = OsStr::new(INCOMING);
    let incoming = root.dir(incoming_name, Some(0o700)).map_err(io_error)?;
    let objects = incoming
        .dir(OsStr::new("objects"), Some(0o700))
        .map_err(io_error)?;
    let work = incoming
        .dir(OsStr::new("work"), Some(0o700))
        .map_err(io_error)?;
    let owned: BTreeSet<_> = inventory
        .rows
        .iter()
        .map(|row| row.path.as_path())
        .chain(runtime.iter().map(|file| file.path.as_path()))
        .collect();
    let mut existing = BTreeSet::new();
    for dir in ["bin", "sbin", "lib", "usr"] {
        list_payload(root, Path::new(dir), &mut existing).map_err(io_error)?;
    }
    let mut changed_paths = Vec::new();
    let mut retire = Vec::new();
    let report_name = OsStr::new("report");
    let report = if work.info(report_name).map_err(io_error)?.is_some() {
        String::from_utf8(work.read(report_name).map_err(io_error)?)
            .map_err(|err| err.to_string())?
    } else {
        let mut carried = Vec::new();
        let mut deleted = Vec::new();
        let mut on_box = Vec::new();
        for (group, yes) in inventory.groups.iter().zip(&valid) {
            if *yes {
                continue;
            }
            for &member in group {
                let row = &inventory.rows[member];
                if let Some((dir, info)) = root.lookup(&row.path).map_err(io_error)? {
                    if row.kind == DIR && info.kind() == DIR {
                        continue;
                    }
                    let fingerprint = fingerprint(&dir, row.leaf(), info).map_err(io_error)?;
                    if old.get(&row.path) != Some(&fingerprint)
                        && !row.matches(&dir, info).map_err(io_error)?
                    {
                        carried.push(display_path(&row.path));
                    }
                }
            }
        }
        for file in &runtime {
            if let Some((RuntimeAction::Carry, _)) =
                runtime_action(root, file, &old, owner).map_err(io_error)?
            {
                carried.push(display_path(&file.path));
            }
        }
        for path in existing
            .iter()
            .filter(|p| !owned.contains(p.as_path()) && checked_retirement(p))
        {
            let (dir, info) = root
                .lookup(path)
                .map_err(io_error)?
                .ok_or("draft path disappeared")?;
            let leaf = path.file_name().unwrap();
            if old.get(path) != Some(&fingerprint(&dir, leaf, info).map_err(io_error)?) {
                on_box.push(display_path(path));
            }
        }
        for path in old
            .keys()
            .filter(|p| checked_retirement(p) && !existing.contains(*p))
        {
            if owned.contains(path.as_path()) {
                println!(
                    "[persistent] restored on-box deletion: {:?}",
                    display_path(path)
                );
            }
            deleted.push(display_path(path));
        }
        let report =
            serde_json::json!({"carried": carried, "deletedOnBox": deleted, "onBox": on_box})
                .to_string();
        work.write(report_name, report.as_bytes(), 0o600)
            .map_err(io_error)?;
        report
    };
    // Before the inventory's carries, retirements and installs, while the uploaded objects
    // and the draft's /lib are still as the upload left them.
    install_runtime(root, &objects, previous, &toolchain, &runtime, &old, owner)?;

    for (group, yes) in inventory.groups.iter().zip(&valid) {
        if *yes {
            continue;
        }
        for &member in group {
            let row = &inventory.rows[member];
            if let Some((dir, info)) = root.lookup(&row.path).map_err(io_error)? {
                if row.kind == DIR && info.kind() == DIR {
                    continue;
                }
                let same = row.matches(&dir, info).map_err(io_error)?;
                let old_identity = fingerprint(&dir, row.leaf(), info).map_err(io_error)?;
                if !same && old.get(&row.path) != Some(&old_identity) {
                    changed_paths.push((&row.path, info));
                }
            }
        }
    }
    for path in existing
        .iter()
        .filter(|p| !owned.contains(p.as_path()) && checked_retirement(p))
    {
        let (dir, info) = root
            .lookup(path)
            .map_err(io_error)?
            .ok_or("draft path disappeared")?;
        if old.get(path)
            == Some(&fingerprint(&dir, path.file_name().unwrap(), info).map_err(io_error)?)
        {
            retire.push((path, info.kind() == DIR));
        }
    }
    for (path, info) in changed_paths {
        carry(root, path, previous, info).map_err(io_error)?;
    }
    for (path, directory) in retire.into_iter().rev() {
        let Some((dir, _)) = root.lookup(path).map_err(io_error)? else {
            continue;
        };
        let leaf = path.file_name().unwrap();
        if !directory
            || dir
                .dir(leaf, None)
                .map_err(io_error)?
                .names()
                .map_err(io_error)?
                .is_empty()
        {
            dir.remove(leaf, directory).map_err(io_error)?;
        }
    }

    let mut last = HashMap::new();
    let mut directories = Vec::new();
    for (index, group) in inventory.groups.iter().enumerate() {
        let row = &inventory.rows[group[0]];
        if row.kind == REG && !valid[index] {
            // Use the last inventory inode even when an earlier one was
            // already published by a previous apply invocation.
            last.insert(&row.digest, index);
        }
        if !valid[index] && row.kind == DIR {
            let (parent, leaf) = root.parent(&row.path, true).map_err(io_error)?;
            let child = parent.dir(leaf, Some(0o755)).map_err(io_error)?;
            tree::owner_mode(&child.file, row.uid, row.gid, row.mode).map_err(io_error)?;
            directories.push(row);
        }
    }
    let mut copied = 0;
    let order = inventory.groups.iter().enumerate();
    for (index, group) in order
        .clone()
        .filter(|(_, g)| inventory.rows[g[0]].kind == LINK)
        .chain(order.filter(|(_, g)| inventory.rows[g[0]].kind == REG))
    {
        let row = &inventory.rows[group[0]];
        if valid[index] {
            continue;
        }
        let anchor_name = format!("inode-{index}");
        let anchor = OsStr::new(&anchor_name);
        if work.info(anchor).map_err(io_error)?.is_none() {
            let mut input = object(&objects, row).map_err(io_error)?;
            if row.kind == LINK {
                let mut target = Vec::new();
                input.read_to_end(&mut target).map_err(io_error)?;
                work.symlink(&target, anchor).map_err(io_error)?;
            } else if last[&row.digest] == index {
                // Keep a restartable private anchor through the last member.
                objects
                    .rename(OsStr::new(&row.digest), &work, anchor)
                    .map_err(io_error)?;
            } else {
                work.atomic(anchor, |temp| {
                    let mut file = work.create(temp)?;
                    let mut buffer = [0u8; 64 * 1024];
                    use std::io::Write as _;
                    loop {
                        let count = input.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        file.write_all(&buffer[..count])?;
                        copied += count as u64;
                        if copied >= MAX_BATCH {
                            root.file.sync_all()?;
                            copied = 0;
                        }
                    }
                    file.sync_all()
                })
                .map_err(io_error)?;
            }
        }
        if row.kind == REG {
            let file = work.read_file(anchor).map_err(io_error)?;
            tree::owner_mode(&file, row.uid, row.gid, row.mode).map_err(io_error)?;
            work.mtime(anchor, row.mtime.0, row.mtime.1)
                .map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
        } else {
            work.symlink_owner(anchor, row.uid, row.gid)
                .map_err(io_error)?;
            work.mtime(anchor, row.mtime.0, row.mtime.1)
                .map_err(io_error)?;
        }
        for &member in group {
            let target = &inventory.rows[member];
            let (parent, leaf) = root.parent(&target.path, true).map_err(io_error)?;
            let info = parent.info(leaf).map_err(io_error)?;
            if info.is_some_and(|info| info.kind() == DIR) {
                parent.remove(leaf, true).map_err(io_error)?;
            }
            if row.kind == LINK {
                let bytes = work.read_link(anchor).map_err(io_error)?;
                parent
                    .atomic(leaf, |tmp| {
                        parent.symlink(&bytes, tmp)?;
                        parent.symlink_owner(tmp, row.uid, row.gid)?;
                        parent.mtime(tmp, row.mtime.0, row.mtime.1)
                    })
                    .map_err(io_error)?;
            } else {
                let anchor_info = work
                    .info(anchor)
                    .map_err(io_error)?
                    .ok_or("lost inventory inode anchor")?;
                if info.is_none_or(|info| info.ino != anchor_info.ino) {
                    parent
                        .atomic(leaf, |tmp| work.link(anchor, &parent, tmp))
                        .map_err(io_error)?;
                }
            }
            checkpoint()?;
        }
        work.remove(anchor, false).map_err(io_error)?;
        work.file.sync_all().map_err(io_error)?;
    }
    for row in directories.into_iter().rev() {
        let (parent, leaf) = root.parent(&row.path, false).map_err(io_error)?;
        parent
            .mtime(leaf, row.mtime.0, row.mtime.1)
            .map_err(io_error)?;
    }
    write_path(root, GENERATION, format!("{id}\n").as_bytes(), 0o644).map_err(io_error)?;
    let mut managed = BTreeMap::new();
    let paths = inventory.rows.iter().map(|row| &row.path);
    for path in paths.chain(runtime.iter().map(|file| &file.path)) {
        let (dir, info) = root
            .lookup(path)
            .map_err(io_error)?
            .ok_or("inventory path disappeared")?;
        managed.insert(
            path.clone(),
            fingerprint(&dir, path.file_name().unwrap(), info).map_err(io_error)?,
        );
    }
    let image = local::baseline_text(managed);
    write_path(root, BASELINE, &image, 0o600).map_err(io_error)?;
    write_path(root, ".complete", layout::record(id).as_bytes(), 0o644).map_err(io_error)?;
    // BreenFS refuses to remove a directory that an open descriptor holds (EBUSY), so
    // close this function's own handles on the incoming tree before removing it.
    drop((objects, work, incoming));
    root.remove_tree(incoming_name).map_err(io_error)?;
    root.file.sync_all().map_err(io_error)?;
    Ok(report)
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn record_fixture(path: &Path) -> Result<Vec<u8>, String> {
    let root = Dir::open(path).map_err(io_error)?;
    let mut paths = BTreeSet::new();
    for dir in ["bin", "sbin", "lib", "usr"] {
        list_payload(&root, Path::new(dir), &mut paths).map_err(io_error)?;
    }
    let mut files = BTreeMap::new();
    for path in paths {
        let (dir, info) = root
            .lookup(&path)
            .map_err(io_error)?
            .ok_or("fixture file missing")?;
        files.insert(
            path.clone(),
            fingerprint(&dir, path.file_name().unwrap(), info).map_err(io_error)?,
        );
    }
    Ok(local::baseline_text(files))
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn have_fixture(path: &Path, bytes: &[u8]) -> Result<Vec<String>, String> {
    let root = Dir::open(path).map_err(io_error)?;
    let inventory = Inventory::parse(bytes)?;
    Ok(inventory
        .have(&root)?
        .into_iter()
        .map(|i| display_path(&inventory.rows[i].path))
        .collect())
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn apply_fixture(
    path: &Path,
    rust: &Path,
    id: &str,
    previous: &str,
    bytes: &[u8],
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<String, String> {
    apply(
        &Dir::open(path).map_err(io_error)?,
        &Dir::open(rust).map_err(io_error)?,
        id,
        previous,
        &Inventory::parse(bytes)?,
        checkpoint,
    )
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn verify_fixture(path: &Path, bytes: &[u8]) -> Result<String, String> {
    Inventory::parse(bytes)?.verify(&Dir::open(path).map_err(io_error)?)
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn select_fixture(path: &Path, id: &str) -> Result<(), String> {
    set_selection(&Dir::open(path).map_err(io_error)?, id).map_err(io_error)
}

fn reclaimable(name: &str, number: u64, retained: &[String], booted: &str) -> bool {
    number != 1
        && name != booted
        && !retained.iter().any(|root| root == name)
        && (name.strip_prefix("gen-").is_some_and(is_metal_generation)
            || name.strip_suffix(".new").is_some_and(is_metal_generation))
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn reclaim_fixture(
    roots: &BTreeMap<String, u64>,
    retained: &[String],
    booted: &str,
) -> Vec<String> {
    roots
        .iter()
        .filter(|(name, number)| reclaimable(name, **number, retained, booted))
        .map(|(name, _)| name.clone())
        .collect()
}

fn epoch_record(epoch: u64, boot: &str, root: &str) -> String {
    format!("{epoch}\n{boot}\n{root}\n")
}

fn read_epoch(bytes: &[u8], boot: &str, root: &str) -> Result<u64, String> {
    let text = std::str::from_utf8(bytes).map_err(|err| err.to_string())?;
    let fields: Vec<_> = text.lines().collect();
    match fields.as_slice() {
        [epoch, boot_id, roots] if *boot_id == boot && *roots == root => epoch
            .parse()
            .map_err(|_| "invalid stored root epoch".into()),
        _ => Err("draft epoch belongs to another boot".into()),
    }
}

enum DraftAction {
    Detach,
    CheckPending,
    Rename,
    Thaw,
    Delete,
}

fn publish_root(
    first: u64,
    freeze: impl FnOnce() -> Result<u64, String>,
    mut action: impl FnMut(DraftAction) -> Result<(), String>,
) -> Result<(), String> {
    action(DraftAction::Detach)?;
    let second = freeze()?;
    if first != second {
        action(DraftAction::Thaw)?;
        action(DraftAction::Delete)?;
        return Err(format!(
            "running root changed during the deploy (epoch {first} -> {second}); rerun"
        ));
    }
    let result = action(DraftAction::CheckPending).and_then(|()| action(DraftAction::Rename));
    if result.is_err() {
        action(DraftAction::Thaw)?;
    }
    result
}

const CURRENT: &str = "CURRENT";
const SELECTION_TARGET: &[u8] = b"/usr/lib/breenix/rust-current";

fn version_id(raw: &[u8]) -> Result<&str, String> {
    let id = std::str::from_utf8(raw).map_err(|err| err.to_string())?;
    let id = id.strip_suffix('\n').unwrap_or(id);
    if id.is_empty()
        || id.len() > 96
        || !id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_'))
    {
        return Err("invalid toolchain selection".into());
    }
    Ok(id)
}

fn convert_selection(
    booted: &Dir,
    rust: &Dir,
    seed_others: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    let name = OsStr::new(CURRENT);
    match rust.info(name).map_err(io_error)?.map(Info::kind) {
        Some(LINK) if rust.read_link(name).map_err(io_error)? == SELECTION_TARGET => {
            version_id(&field(booted, SELECTION).map_err(io_error)?)?;
        }
        Some(REG) => {
            let old = rust.read(name).map_err(io_error)?;
            let id = version_id(&old)?;
            seed_others(id)?;
            set_selection(booted, id).map_err(io_error)?;
            rust.atomic(name, |tmp| rust.symlink(SELECTION_TARGET, tmp))
                .map_err(io_error)?;
        }
        _ => {
            return Err("/opt/breenix/rust/CURRENT is not the supported toolchain selection".into())
        }
    }
    Ok(())
}

fn seed_selection(root: &Dir, id: &str, retained: &str, writable: bool) -> Result<(), String> {
    if !writable {
        return Err(format!("retained root {retained} is read-only"));
    }
    if root
        .lookup(Path::new(SELECTION))
        .map_err(io_error)?
        .is_none()
    {
        set_selection(root, id).map_err(io_error)?;
    } else {
        version_id(&field(root, SELECTION).map_err(io_error)?)?;
    }
    root.file.sync_all().map_err(io_error)
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn convert_fixture(
    booted: &Path,
    opt: &Path,
    retained: &[(&Path, bool)],
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let root = Dir::open(booted).map_err(io_error)?;
    let rust = Dir::open(opt)
        .and_then(|opt| {
            opt.dir(OsStr::new("breenix"), None)?
                .dir(OsStr::new("rust"), None)
        })
        .map_err(io_error)?;
    convert_selection(&root, &rust, |id| {
        for (path, writable) in retained {
            seed_selection(
                &Dir::open(path).map_err(io_error)?,
                id,
                "PREVIOUS",
                *writable,
            )?;
            checkpoint()?;
        }
        Ok(())
    })
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn verify_runtime_fixture(path: &Path, rust: &Path) -> Result<(), String> {
    toolchain(
        &Dir::open(path).map_err(io_error)?,
        &Dir::open(rust).map_err(io_error)?,
    )
}

/// The toolchain version the draft selects: its ID, its directory in shared
/// /opt/breenix/rust and its parsed MANIFEST.json.
fn selected_manifest(root: &Dir, rust: &Dir) -> Result<(String, Dir, serde_json::Value), String> {
    let selected = field(root, SELECTION).map_err(|err| format!("/{SELECTION}: {err}"))?;
    let id = version_id(&selected)?;
    let place = format!("/opt/breenix/rust/{id}");
    let version = rust
        .dir(OsStr::new(id), None)
        .map_err(|err| format!("{place}: {err}"))?;
    let bytes = version
        .read(OsStr::new("MANIFEST.json"))
        .map_err(|err| format!("{place}/MANIFEST.json: {err}"))?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
    if manifest["id"].as_str() != Some(id) {
        return Err(format!("toolchain manifest does not identify {id}"));
    }
    Ok((id.to_owned(), version, manifest))
}

/// The manifest's runtime rows as `(<name>, facts)`, each naming one file directly in /lib.
fn runtime_rows(manifest: &serde_json::Value) -> Result<Vec<(&str, &serde_json::Value)>, String> {
    let runtime = manifest["runtime"]
        .as_object()
        .ok_or("missing toolchain runtime inventory")?;
    // scripts/devbox/toolchain.py keys each runtime row by its absolute path,
    // `/lib/<name>`; the check reads it as `<name>` inside the draft's /lib.
    for required in ["/lib/libc.so", "/lib/ld-breenix.so"] {
        if !runtime.contains_key(required) {
            return Err(format!("missing toolchain runtime {required}"));
        }
    }
    runtime
        .iter()
        .map(|(path, facts)| {
            path.strip_prefix("/lib/")
                .filter(|name| {
                    !name.is_empty() && !name.contains('/') && !matches!(*name, "." | "..")
                })
                .map(|name| (name, facts))
                .ok_or_else(|| format!("unsafe toolchain runtime path {path:?}"))
        })
        .collect()
}

/// One runtime file the draft's /lib must hold, from the selected toolchain's entry.
struct Runtime {
    path: PathBuf,
    size: u64,
    mode: u32,
    sha: String,
}

/// What the runtime step does with one entry's path in the draft (design 4.3's rules).
#[derive(PartialEq)]
enum RuntimeAction {
    Keep,
    Replace,
    Carry,
}

/// DEV-55: the selected toolchain's runtime entry is the source of truth for the draft's
/// /lib runtime. The draft keeps the clone's selection unless the inventory carries one, and
/// an inventory that carries the selection, or a /lib runtime path, owns that path itself.
fn runtime_plan(
    root: &Dir,
    rust: &Dir,
    inventory: &Inventory,
) -> Result<(String, Vec<Runtime>), String> {
    let carries = |path: &Path| inventory.rows.iter().any(|row| row.path == path);
    // Begin writes the selection before the clone (4.5), so a draft without one only
    // occurs outside a deploy; verify refuses it, naming the selection.
    if carries(Path::new(SELECTION))
        || root
            .lookup(Path::new(SELECTION))
            .map_err(io_error)?
            .is_none()
    {
        return Ok((String::new(), Vec::new()));
    }
    let (id, _, manifest) = selected_manifest(root, rust)?;
    let mut plan = Vec::new();
    for (name, facts) in runtime_rows(&manifest)? {
        let path = Path::new("lib").join(name);
        if carries(&path) {
            continue;
        }
        let size = facts["size"].as_u64();
        let mode = facts["mode"]
            .as_str()
            .and_then(|mode| u32::from_str_radix(mode, 8).ok())
            .filter(|mode| *mode <= 0o7777);
        let sha = facts["sha256"]
            .as_str()
            .filter(|sha| sha.len() == 64 && is_hex(sha));
        let (Some(size), Some(mode), Some(sha)) = (size, mode, sha) else {
            return Err(format!(
                "toolchain {id} has an invalid runtime row for /lib/{name}"
            ));
        };
        plan.push(Runtime {
            path,
            size,
            mode,
            sha: sha.into(),
        });
    }
    Ok((id, plan))
}

fn runtime_action(
    root: &Dir,
    runtime: &Runtime,
    old: &BTreeMap<PathBuf, String>,
    owner: (u32, u32),
) -> io::Result<Option<(RuntimeAction, Info)>> {
    let Some((dir, info)) = root.lookup(&runtime.path)? else {
        return Ok(None);
    };
    let leaf = runtime.path.file_name().expect("validated runtime path");
    if info.kind() == REG
        && info.size == runtime.size
        && info.mode & 0o7777 == runtime.mode
        && (info.uid, info.gid) == owner
        && info.links == 1
        && digest_file(dir.read_file(leaf)?)? == runtime.sha
    {
        return Ok(Some((RuntimeAction::Keep, info)));
    }
    let action =
        if info.kind() == REG && old.get(&runtime.path) == Some(&fingerprint(&dir, leaf, info)?) {
            RuntimeAction::Replace
        } else {
            RuntimeAction::Carry
        };
    Ok(Some((action, info)))
}

/// Bytes with the entry's size and SHA-256: the uploaded object named by that SHA-256, or a
/// regular file already in the draft's /lib (on the R240, the pinned loader the diagnostic
/// delivery kept). Only the bytes are taken, by content; the name does not matter.
fn runtime_source(lib: &Dir, objects: &Dir, runtime: &Runtime) -> io::Result<Option<File>> {
    let mut candidates = vec![(objects, OsString::from(&runtime.sha))];
    candidates.extend(lib.names()?.into_iter().map(|name| (lib, name)));
    for (dir, name) in candidates {
        let Some(info) = dir.info(&name)? else {
            continue;
        };
        if info.kind() == REG
            && info.size == runtime.size
            && digest_file(dir.read_file(&name)?)? == runtime.sha
        {
            return dir.read_file(&name).map(Some);
        }
    }
    Ok(None)
}

/// Write each planned runtime file into the draft, moving a locally changed one aside first.
/// Each source is opened before anything moves, and the copy is checked against the entry's
/// SHA-256 before it is renamed in. Rerunning after an interruption keeps what is finished.
fn install_runtime(
    root: &Dir,
    objects: &Dir,
    previous: &str,
    toolchain: &str,
    plan: &[Runtime],
    old: &BTreeMap<PathBuf, String>,
    owner: (u32, u32),
) -> Result<(), String> {
    for runtime in plan {
        let action = runtime_action(root, runtime, old, owner).map_err(io_error)?;
        if matches!(action, Some((RuntimeAction::Keep, _))) {
            continue;
        }
        let (lib, leaf) = root.parent(&runtime.path, true).map_err(io_error)?;
        let shown = display_path(&runtime.path);
        let mut source = runtime_source(&lib, objects, runtime)
            .map_err(io_error)?
            .ok_or_else(|| {
                format!(
                    "no object or file in the draft's /lib matches toolchain {toolchain}'s {shown} \
                     (SHA-256 {}, {} bytes)",
                    runtime.sha, runtime.size
                )
            })?;
        if let Some((RuntimeAction::Carry, info)) = action {
            carry(root, &runtime.path, previous, info).map_err(io_error)?;
        }
        lib.atomic(leaf, |temp| {
            let result = (|| {
                let mut output = lib.create(temp)?;
                let mut hash = local::sha256::Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                let mut size = 0u64;
                use std::io::Write as _;
                loop {
                    let count = source.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                    output.write_all(&buffer[..count])?;
                    size += count as u64;
                }
                if size != runtime.size || local::hex(hash.finalize()) != runtime.sha {
                    return Err(io::Error::other(format!(
                        "runtime source changed for {shown}"
                    )));
                }
                tree::owner_mode(&output, owner.0, owner.1, runtime.mode)?;
                output.sync_all()
            })();
            if result.is_err() {
                let _ = lib.remove(temp, false);
            }
            result
        })
        .map_err(io_error)?;
    }
    Ok(())
}

fn toolchain(root: &Dir, rust: &Dir) -> Result<(), String> {
    let (id, version, manifest) = selected_manifest(root, rust)?;
    let id = id.as_str();
    let payload = manifest["files"]
        .as_object()
        .filter(|files| !files.is_empty())
        .ok_or("empty toolchain file inventory")?;
    let runtime = runtime_rows(&manifest)?;
    let payload: Vec<_> = payload.iter().map(|(path, facts)| (path.as_str(), facts)).collect();
    let libraries = root
        .dir(OsStr::new("lib"), None)
        .map_err(|err| format!("/lib: {err}"))?;
    for (root, rows, hash) in [(&version, payload, false), (&libraries, runtime, true)] {
        for (path, facts) in rows {
            let path = Path::new(path);
            let shown = if hash {
                Path::new("/lib").join(path)
            } else {
                path.to_path_buf()
            };
            if path.is_absolute()
                || path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                || hash && path.components().count() != 1
            {
                return Err(format!("unsafe toolchain manifest path {shown:?}"));
            }
            let Some((dir, info)) = root.lookup(path).map_err(io_error)? else {
                return Err(format!("missing toolchain file {shown:?}"));
            };
            let mode = facts["mode"]
                .as_str()
                .and_then(|mode| u32::from_str_radix(mode, 8).ok());
            let sha = facts["sha256"]
                .as_str()
                .filter(|sha| sha.len() == 64 && is_hex(sha));
            if info.kind() != REG
                || Some(info.size) != facts["size"].as_u64()
                || Some(info.mode & 0o7777) != mode
                || sha.is_none()
                || hash
                    && digest_file(dir.read_file(path.file_name().unwrap()).map_err(io_error)?)
                        .map_err(io_error)?
                        != sha.unwrap()
            {
                return Err(format!(
                    "toolchain {id} metadata or runtime bytes differ at {shown:?}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(feature = "paired-host-test")]
pub(crate) fn publish_fixture(
    state: &[u8],
    boot: &str,
    root: &str,
    epoch: u64,
    mut action: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let first = read_epoch(state, boot, root)?;
    publish_root(
        first,
        || Ok(epoch),
        |op| {
            action(match op {
                DraftAction::Detach => "detach",
                DraftAction::CheckPending => "pending",
                DraftAction::Rename => "rename",
                DraftAction::Thaw => "thaw",
                DraftAction::Delete => "delete",
            })
        },
    )
}
