//! Stage a userland beside the running install, then publish it with one rename.
//! The kernel slot embeds the generation ID, so no independently mutable current
//! pointer can pair a kernel with the wrong userland. v2 binds `/etc`, `/var`,
//! `/opt`, `/srv`, `/tmp` and `/run` from `/.breenix-shared`; `/root` stays at the
//! partition root. The first v2 install seeds from the previous slot's v1 tree.
//! The operator account
//! `metal-run install-persistent` stages on the test root goes into the store,
//! not into the generation. A test image's SSH credentials (its test-only
//! client key, its test account and `/root/.ssh`) stay behind. This
//! program mounts no home partition.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[path = "../../../libs/libbreenix/src/layout.rs"]
mod layout;

const TARGET: &str = "/home";
const KEPT: [&str; 1] = ["root"];
const UNTOUCHED: [&str; 8] = ["home", "lost+found", "proc", "dev", "sys", ".breenix-generations", ".complete", ".breenix-shared"];
const GENERATIONS: &str = ".breenix-generations";
const READY: &str = ".complete";
/// The machine state store each generation binds (kernel/src/fs/ext2/generation.rs).
const STORE: &str = ".breenix-shared";
/// The operator account the boot host stages on the test root for this program:
/// a `user <name>` line and a `key <public key line>` line.
const OPERATOR: &str = "etc/breenix/operator-account";
/// The operator account's uid and gid: the first ordinary account's.
const OPERATOR_ID: u32 = 1000;
/// The account records a test image's fixtures added (scripts/install-layout.sh):
/// `passwd|group|shadow <record>` for each record, verbatim, and
/// `member <group> <account>` for each fixture membership of a system group.
/// The install leaves out exactly these records, and a membership only with
/// its account's own record; an account is not left out because of its name.
const FIXTURES: &str = "etc/breenix/test-fixtures";
/// Source paths the copy leaves behind: the test image's SSH client key and
/// test account home, the fixture accounts' homes and records, and the staged
/// operator account.
const LEFT_BEHIND: [&str; 4] = ["etc/breenix/test-ssh", "etc/breenix/test-users", FIXTURES, OPERATOR];
/// Inside a copied `/root`: a test image's authorized keys are not the install's.
const ROOT_LEFT_BEHIND: &str = "root/.ssh";

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
    /// Source paths copy_tree passes over.
    skip: Vec<PathBuf>,
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

    /// Replace `path` with `text` at `mode`: written beside it, flushed, renamed over it.
    fn write_file(&mut self, path: &Path, text: &str, mode: u32) -> Result<(), String> {
        let mut name = path.file_name().unwrap_or_default().to_owned();
        name.push(".new");
        let staging = path.with_file_name(name);
        if present(&staging)? {
            self.change("remove", &staging, || fs::remove_file(&staging))?;
        }
        let mut file = self.change("create", &staging, || File::create(&staging))?;
        // The mode is set before any bytes are written, so others cannot read private text.
        self.change("chmod", &staging, || fs::set_permissions(&staging, fs::Permissions::from_mode(mode)))?;
        self.change("write", &staging, || file.write_all(text.as_bytes()))?;
        self.change("sync file", &staging, || file.sync_all())?;
        drop(file);
        self.change("rename", path, || fs::rename(&staging, path))?;
        self.sync(path.parent().unwrap_or(Path::new("/")))
    }

    /// Create `path` as a directory at `mode` unless it is one already.
    fn ensure_directory(&mut self, path: &Path, mode: u32) -> Result<(), String> {
        if !present(path)? {
            self.change("mkdir", path, || fs::create_dir(path))?;
        }
        require_directory(path)?;
        // Retry the mode and durability steps even if mkdir completed before a reset.
        self.change("chmod", path, || fs::set_permissions(path, fs::Permissions::from_mode(mode)))?;
        self.sync(path)?;
        self.sync(path.parent().unwrap_or(Path::new("/")))
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
                let child = source.join(&name);
                if self.skip.contains(&child) {
                    println!("[persistent] left out {}: test or staging credentials", child.display());
                    continue;
                }
                if usable(&name, source, &mut self.totals) {
                    self.copy_tree(&child, &target.join(&name))?;
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

/// The operator account staged on the test root.
struct Operator {
    name: String,
    key: String,
}

/// An account name: a lowercase letter or `_`, then up to 31 lowercase letters, digits, `_` or `-`.
fn valid_account_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty() && bytes.len() <= 32
        && (bytes[0].is_ascii_lowercase() || bytes[0] == b'_')
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
}

/// One `ssh-ed25519` or `ssh-rsa` public key line: type, base64 key, optional comment.
fn valid_public_key(line: &str) -> bool {
    let mut words = line.split(' ');
    matches!(words.next(), Some("ssh-ed25519" | "ssh-rsa"))
        && words.next().is_some_and(|key| !key.is_empty()
            && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='))
        && line.bytes().all(|b| (0x20..0x7f).contains(&b))
}

fn read_operator(source: &Path) -> Result<Option<Operator>, String> {
    let path = source.join(OPERATOR);
    if !present(&path)? {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|error| fail("read", &path, error))?;
    let (mut name, mut key) = (None, None);
    for line in text.lines() {
        match line.split_once(' ') {
            Some(("user", value)) if name.is_none() => name = Some(value.to_string()),
            Some(("key", value)) if key.is_none() => key = Some(value.to_string()),
            _ => return Err(format!("{}: unexpected line", path.display())),
        }
    }
    match (name, key) {
        (Some(name), Some(key)) if valid_account_name(&name) && name != "root" && valid_public_key(&key) => {
            Ok(Some(Operator { name, key }))
        }
        _ => Err(format!("{} must name a non-root account and hold one public key line", path.display())),
    }
}

/// `existing` with `entry` in place of the line naming the same account, or appended.
fn merge_line(existing: &str, entry: &str) -> String {
    let name = entry.split(':').next().unwrap_or("");
    let mut lines: Vec<&str> = existing.lines().collect();
    match lines.iter().position(|line| line.split(':').next() == Some(name)) {
        Some(index) => lines[index] = entry,
        None => lines.push(entry),
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn read_or_empty(path: &Path) -> Result<String, String> {
    if !present(path)? {
        return Ok(String::new());
    }
    fs::read_to_string(path).map_err(|error| fail("read", path, error))
}

/// Create the machine state store, private to root, and put the operator's
/// account lines and staged key in it (machine_state.rs in libbreenix's ssh module reads them).
fn stage_store(copier: &mut Copier, target: &Path, operator: Option<&Operator>) -> Result<(), String> {
    let store = target.join(STORE);
    copier.ensure_directory(&store, 0o700)?;
    let Some(operator) = operator else { return Ok(()) };
    let etc = store.join("etc");
    copier.ensure_directory(&etc, 0o755)?;
    let name = &operator.name;
    let passwd = read_or_empty(&etc.join("passwd"))?;
    if passwd.lines().any(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        fields.len() == 7 && fields[0] != name && fields[2] == OPERATOR_ID.to_string()
    }) {
        return Err(format!("the store already gives uid {} to another account", OPERATOR_ID));
    }
    let entries = [
        ("passwd", passwd, format!("{0}:x:{1}:{1}:{0}:/home/{0}:/bin/bsh", name, OPERATOR_ID), 0o644),
        ("group", read_or_empty(&etc.join("group"))?, format!("{}:x:{}:", name, OPERATOR_ID), 0o644),
        // A locked password: the account logs in with its SSH key.
        ("shadow", read_or_empty(&etc.join("shadow"))?, format!("{}:!:::::::", name), 0o600),
    ];
    for (file, existing, entry, mode) in entries {
        let has_line = existing.lines().any(|line| line.split(':').next() == Some(name.as_str()));
        // Existing machine entries may have been edited since installation.
        let merged = if file == "shadow" && has_line { existing.clone() } else { merge_line(&existing, &entry) };
        if merged != existing {
            copier.write_file(&etc.join(file), &merged, mode)?;
        }
    }
    let mut pending = store.clone();
    for (part, mode) in [("var", 0o755), ("lib", 0o755), ("accounts", 0o700), ("pending", 0o700)] {
        pending.push(part);
        copier.ensure_directory(&pending, mode)?;
    }
    copier.write_file(&pending.join(name), &format!("{}\n", operator.key), 0o600)?;
    println!("[persistent] staged account {} (uid {}) and its SSH public key in /{}", name, OPERATOR_ID, STORE);
    Ok(())
}

/// The records a test image's fixtures added (`FIXTURES`); empty for an image
/// without the file.
#[derive(Default)]
struct Fixtures {
    /// (file, record)
    records: Vec<(String, String)>,
    /// (group, account)
    members: Vec<(String, String)>,
}

fn read_fixtures(source: &Path) -> Result<Fixtures, String> {
    let path = source.join(FIXTURES);
    let mut fixtures = Fixtures::default();
    if !present(&path)? {
        return Ok(fixtures);
    }
    let text = fs::read_to_string(&path).map_err(|error| fail("read", &path, error))?;
    for line in text.lines() {
        match line.split_once(' ') {
            Some((file @ ("passwd" | "group" | "shadow"), record)) if record.contains(':') => {
                fixtures.records.push((file.to_string(), record.to_string()));
            }
            Some(("member", pair)) => match pair.split_once(' ') {
                Some((group, account)) if !group.is_empty() && !account.is_empty() && !account.contains([' ', ',']) => {
                    fixtures.members.push((group.to_string(), account.to_string()));
                }
                _ => return Err(format!("/{}: malformed membership {:?}", FIXTURES, line)),
            },
            _ => return Err(format!("/{}: malformed record {:?}", FIXTURES, line)),
        }
    }
    Ok(fixtures)
}

/// Leave the test image's fixture records out of the account files in `etc`:
/// each record that is exactly a fixture's, and a fixture membership of a
/// system group where that account's own fixture record was there. A record the
/// machine made or changed differs from the fixture's and stays, whatever its name.
fn drop_test_account(copier: &mut Copier, etc: &Path, fixtures: &Fixtures) -> Result<(), String> {
    let passwd = read_or_empty(&etc.join("passwd"))?;
    let removed: Vec<&str> = fixtures
        .records
        .iter()
        .filter(|(file, record)| file == "passwd" && passwd.lines().any(|line| line == record))
        .filter_map(|(_, record)| record.split(':').next())
        .collect();
    for file in ["passwd", "group", "shadow"] {
        let path = etc.join(file);
        if !present(&path)? {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|error| fail("read", &path, error))?;
        let kept: Vec<String> = text
            .lines()
            .filter(|line| !fixtures.records.iter().any(|(f, record)| f == file && record == line))
            .map(|line| without_test_members(file, line, fixtures, &removed))
            .collect();
        if kept.iter().map(String::as_str).ne(text.lines()) {
            let mode = fs::symlink_metadata(&path).map_err(|error| fail("stat", &path, error))?.mode() & 0o7777;
            let mut kept = kept.join("\n");
            kept.push('\n');
            copier.write_file(&path, &kept, mode)?;
            println!("[persistent] left out the test fixtures' records from /etc/{}", file);
        }
    }
    Ok(())
}

/// A group line without the fixture memberships of accounts in `removed`;
/// other lines as they are.
fn without_test_members(file: &str, line: &str, fixtures: &Fixtures, removed: &[&str]) -> String {
    let fields: Vec<&str> = line.split(':').collect();
    if file != "group" || fields.len() != 4 {
        return line.to_string();
    }
    let fixture = |member: &str| {
        removed.contains(&member) && fixtures.members.iter().any(|(group, account)| group == fields[0] && account == member)
    };
    let members: Vec<&str> = fields[3].split(',').filter(|m| !m.is_empty() && !fixture(m)).collect();
    if members.join(",") == fields[3] {
        return line.to_string();
    }
    format!("{}:{}:{}:{}", fields[0], fields[1], fields[2], members.join(","))
}

/// Seed missing names through flushed scratch files and rename. Existing names
/// keep the machine state published by an earlier attempt.
fn seed_missing(copier: &mut Copier, source: &Path, target: &Path) -> Result<(), String> {
    if copier.skip.contains(&source.to_path_buf()) { return Ok(()); }
    let meta = fs::symlink_metadata(source).map_err(|e| fail("stat seed", source, e))?;
    if meta.is_dir() {
        copier.ensure_directory(target, meta.mode() & 0o7777)?;
        for entry in fs::read_dir(source).map_err(|e| fail("read seed", source, e))? {
            let entry = entry.map_err(|e| fail("read seed", source, e))?;
            seed_missing(copier, &entry.path(), &target.join(entry.file_name()))?;
        }
    } else if !present(target)? {
        let scratch = target.with_file_name(format!(".{}.seed-new", target.file_name().unwrap().to_string_lossy()));
        if present(&scratch)? { copier.change("remove seed scratch", &scratch, || fs::remove_file(&scratch))?; }
        copier.linked.clear();
        copier.copy_tree(source, &scratch)?;
        copier.change("publish seed", target, || fs::rename(&scratch, target))?;
        copier.sync(target.parent().unwrap())?;
    }
    Ok(())
}

fn seed_shared(copier: &mut Copier, source: &Path, target: &Path, previous: Option<&str>, fixtures: &Fixtures) -> Result<(), String> {
    let store = target.join(STORE);
    copier.ensure_directory(&store, 0o700)?;
    let marker = store.join(".layout-v2");
    if present(&marker)? {
        if fs::read(&marker).map_err(|e| fail("read layout", &marker, e))? != b"BREENIX-SHARED 2\n" {
            return Err("invalid shared layout record".into());
        }
        for name in layout::SHARED { require_directory(&store.join(name))?; }
        return Ok(());
    }
    let current = if let Some(id) = previous {
        if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("invalid previous userland generation".into());
        }
        let current = target.join(GENERATIONS).join(id);
        let record = fs::read(current.join(READY)).map_err(|e| fail("read previous generation", &current, e))?;
        if record != format!("BREENIX-USERLAND 1\n{}\n", id).as_bytes() {
            return Err("first v2 seed requires a complete v1 current generation".into());
        }
        current
    } else if present(&target.join("etc"))? { target.to_path_buf() } else {
        if present(&target.join(GENERATIONS))? {
            return Err("first v2 seed requires the current userland generation".into());
        }
        source.to_path_buf()
    };
    for name in layout::SHARED {
        let from = current.join(name);
        let to = store.join(name);
        // Existing ssh-doors databases contain only the machine's account lines.
        // Merge them over the running generation's full databases before binding /etc.
        if name == "etc" {
            copier.ensure_directory(&to, 0o755)?;
            for (file, mode) in [("passwd", 0o644), ("group", 0o644), ("shadow", 0o600)] {
                let baseline = read_or_empty(&from.join(file))?;
                let existing = read_or_empty(&to.join(file))?;
                let mut merged = baseline;
                for line in existing.lines() { merged = merge_line(&merged, line); }
                if merged != existing { copier.write_file(&to.join(file), &merged, mode)?; }
            }
        }
        if present(&from)? { seed_missing(copier, &from, &to)?; }
        else { copier.ensure_directory(&to, if name == "tmp" { 0o1777 } else { 0o755 })?; }
    }
    drop_test_account(copier, &store.join("etc"), fixtures)?;
    copier.write_file(&marker, "BREENIX-SHARED 2\n", 0o600)
}

fn install_tree(source: &Path, target: &Path, id: &str, checkpoint: &mut dyn FnMut() -> std::io::Result<()>) -> Result<Totals, String> {
    install_tree_from(source, target, id, None, checkpoint)
}

fn install_tree_from(source: &Path, target: &Path, id: &str, previous: Option<&str>, checkpoint: &mut dyn FnMut() -> std::io::Result<()>) -> Result<Totals, String> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(String::from("invalid userland generation"));
    }
    require_directory(target)?;
    let operator = read_operator(source)?;
    let fixtures = read_fixtures(source)?;
    let skip = LEFT_BEHIND.iter().chain([ROOT_LEFT_BEHIND].iter()).map(|path| source.join(path)).collect();
    let mut copier = Copier { totals: Totals::default(), skip, linked: HashMap::new(), buffer: vec![0u8; 64 * 1024], checkpoint };
    let generations = target.join(GENERATIONS);
    let staged = generations.join(format!("{}.new", id));
    let published = generations.join(id);
    if present(&published)? {
        require_directory(&published)?;
        let expected = layout::record(id);
        let ready = published.join(READY);
        if fs::symlink_metadata(&ready).map_err(|error| fail("stat", &ready, error))?.is_file()
            && fs::read(&ready).map_err(|error| fail("read", &ready, error))? == expected.as_bytes() {
            for name in ["root", "home", "dev", "proc", "sys"].into_iter().chain(layout::SHARED) {
                require_directory(&published.join(name))?;
            }
            for name in layout::SHARED { require_directory(&target.join(STORE).join(name))?; }
            require_directory(&target.join("root"))?;
            return Ok(Totals::default());
        }
        return Err(String::from("incomplete published userland generation"));
    }
    // The store is outside the generations: a retry stages the account again, and
    // an interrupted install leaves the current generation as it was.
    // Check the account collision before publishing any layout state.
    stage_store(&mut copier, target, operator.as_ref())?;
    seed_shared(&mut copier, source, target, previous, &fixtures)?;
    for name in KEPT {
        require_directory(&source.join(name))?;
        if present(&target.join(name))? { require_directory(&target.join(name))?; }
    }
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
    for name in names.iter().filter(|name| !UNTOUCHED.contains(&name.as_str()) && !KEPT.contains(&name.as_str()) && !layout::SHARED.contains(&name.as_str())) {
        copier.copy_tree(&source.join(name), &staged.join(name))?;
    }
    // The generation keeps factory defaults; /etc is a shared mount point.
    let factory = staged.join("usr/share/factory/etc");
    if present(&factory)? { drop_test_account(&mut copier, &factory, &fixtures)?; }
    if present(&staged.join("usr/share/factory"))? {
        copier.write_file(&staged.join("usr/share/factory/generation"), id, 0o644)?;
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
    for name in ["root", "home", "dev", "proc", "sys"].into_iter().chain(layout::SHARED) {
        let mount = staged.join(name);
        copier.change("mkdir mount", &mount, || fs::create_dir(&mount))?;
    }
    // The same table used by the image builder supplies the mount-point modes.
    for line in layout::TABLE.lines() {
        let row: Vec<_> = line.split_whitespace().collect();
        if row.len() == 5 && row[0] == "dir" && !row[1].contains('/') {
            let path = staged.join(row[1]);
            if present(&path)? {
                let mode = u32::from_str_radix(row[2], 8).map_err(|_| "invalid layout mode")?;
                copier.change("chmod layout", &path, || fs::set_permissions(&path, fs::Permissions::from_mode(mode)))?;
            }
        }
    }
    let ready = staged.join(READY);
    let record = layout::record(id);
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
    let totals = if let Some(previous) = std::env::args().nth(2) {
        install_tree_from(source, target, &id, Some(&previous), &mut || Ok(()))
    } else { install_tree(source, target, &id, &mut || Ok(())) }?;
    // Retained rollback trees are subject to the same allowlist as the new tree.
    for entry in fs::read_dir(target.join(GENERATIONS)).map_err(|e| fail("read generations", target, e))? {
        let tree = entry.map_err(|e| fail("read generation", target, e))?.path();
        require_directory(&tree)?;
        layout::check_setid(&tree, false)?;
    }
    println!("[persistent] layout set-ID check passed");
    Ok(totals)
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
        assert_eq!(tree.path(namespace.shared(namespace.root, b"etc").unwrap()), system.join(STORE).join("etc"));
        for name in layout::SHARED {
            let bound = namespace.shared(namespace.root, name.as_bytes()).unwrap();
            assert_eq!(tree.path(bound), system.join(STORE).join(name));
            assert_eq!(namespace.entry(bound, b"..", 2), Some(namespace.root));
            assert_eq!(namespace.mount_name(bound), Some(name.as_bytes()));
            assert!(namespace.mount_entry(namespace.root, name.as_bytes()));
        }
        assert_eq!(tree.path(namespace.shared(namespace.root, b"root").unwrap()), system.join("root"));
        assert_eq!(namespace.shared(namespace.root, b"home"), None);
        assert!(namespace.is_mount(namespace.etc));
        tree.path(namespace.root)
    }

    #[test]
    fn v2_seeds_the_named_v1_generation_and_keeps_v1_frozen() {
        let fixture = Fixture::new();
        let system = fixture.system();
        let old = system.join(GENERATIONS).join("old");
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::rename(fixture.source("old"), &old).unwrap();
        fs::write(old.join(READY), b"BREENIX-USERLAND 1\nold\n").unwrap();
        fs::write(old.join("etc/config"), b"live v1 edit").unwrap();
        fs::create_dir(old.join("var")).unwrap();
        fs::write(old.join("var/log-sentinel"), b"v1 log").unwrap();
        fs::create_dir_all(system.join(STORE).join("etc/ssh")).unwrap();
        fs::write(system.join(STORE).join("etc/ssh/ssh_host_ed25519_key"), b"host key").unwrap();
        fs::write(system.join(STORE).join("etc/passwd"), b"operator:x:1000:1000::/home/operator:/bin/sh\n").unwrap();
        let frozen = snapshot(&old);
        let source = fixture.source("new");
        install_tree_from(&source, &system, "new", Some("old"), &mut || Ok(())).unwrap();
        assert_eq!(fs::read(system.join(STORE).join("etc/config")).unwrap(), b"live v1 edit");
        assert_eq!(fs::read(system.join(STORE).join("var/log-sentinel")).unwrap(), b"v1 log");
        assert_eq!(fs::read(system.join(STORE).join("etc/ssh/ssh_host_ed25519_key")).unwrap(), b"host key");
        let tree = Tree::new(&system);
        let v1 = generation::select(&tree, "old").unwrap();
        assert_eq!(tree.path(v1.etc), old.join("etc"));
        assert_eq!(v1.mutable, [0; 6]);
        let v2 = generation::select(&tree, "new").unwrap();
        assert_eq!(tree.path(v2.etc), system.join(STORE).join("etc"));
        assert_eq!(snapshot(&old), frozen);
        fs::write(system.join(STORE).join("etc/config"), b"v2 edit").unwrap();
        install_tree_from(&fixture.source("again"), &system, "again", Some("new"), &mut || Ok(())).unwrap();
        assert_eq!(fs::read(system.join(STORE).join("etc/config")).unwrap(), b"v2 edit");
        assert_eq!(snapshot(&old), frozen);
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

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOperatorPublicKeyBytesForTheHostTestOnly0 operator@boot-host";

    #[test]
    fn retry_repairs_store_mode_after_interrupted_mkdir() {
        let fixture = Fixture::new();
        let path = fixture.0.join(STORE);
        let mut checkpoint = || Ok(());
        let mut copier = Copier {
            totals: Totals::default(), skip: Vec::new(), linked: HashMap::new(),
            buffer: Vec::new(), checkpoint: &mut checkpoint,
        };
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        copier.ensure_directory(&path, 0o700).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
        fs::remove_dir(&path).unwrap();
        std::os::unix::fs::symlink(&fixture.0, &path).unwrap();
        assert!(copier.ensure_directory(&path, 0o700).is_err());
    }

    #[test]
    fn operator_and_host_keys_live_in_the_store_and_test_credentials_stay_behind() {
        let fixture = Fixture::new();
        let source = fixture.source("new");
        fs::create_dir_all(source.join("etc/breenix/test-ssh/home")).unwrap();
        fs::write(source.join("etc/breenix/test-ssh/id_ed25519"), b"test-only private key").unwrap();
        fs::write(source.join(OPERATOR), format!("user operator\nkey {}\n", KEY)).unwrap();
        fs::create_dir_all(source.join("etc/breenix/test-users/ua")).unwrap();
        fs::write(source.join("etc/passwd"), "root:x:0:0:root:/root:/bin/sh\nsshtest:x:2000:2000:sshtest:/etc/breenix/test-ssh/home:/bin/sh\nua:x:2001:2001:Users suite fixture:/etc/breenix/test-users/ua:/bin/bsh\n").unwrap();
        fs::write(source.join("etc/group"), "root:x:0:ua\nsshtest:x:2000:\nua:x:2001:\nug:x:2100:ua\n").unwrap();
        fs::write(source.join(FIXTURES), "passwd sshtest:x:2000:2000:sshtest:/etc/breenix/test-ssh/home:/bin/sh\n\
            passwd ua:x:2001:2001:Users suite fixture:/etc/breenix/test-users/ua:/bin/bsh\n\
            group sshtest:x:2000:\ngroup ua:x:2001:\ngroup ug:x:2100:ua\nmember root ua\n").unwrap();
        fs::create_dir(source.join("root/.ssh")).unwrap();
        fs::write(source.join("root/.ssh/authorized_keys"), b"ssh-ed25519 AAAA test-image").unwrap();
        // A fresh system partition: /root is copied from the image, without its .ssh.
        let system = fixture.0.join("fresh");
        fs::create_dir(&system).unwrap();
        install_tree(&source, &system, "first", &mut || Ok(())).unwrap();
        let root = selected(&system, "first");
        assert!(!root.join("etc/breenix/test-ssh").exists());
        assert!(!root.join("etc/breenix/test-users").exists());
        assert!(!root.join(FIXTURES).exists());
        assert!(!root.join(OPERATOR).exists());
        assert_eq!(fs::read_to_string(system.join(STORE).join("etc/passwd")).unwrap(), "root:x:0:0:root:/root:/bin/sh\noperator:x:1000:1000:operator:/home/operator:/bin/bsh\n");
        assert_eq!(fs::read_to_string(system.join(STORE).join("etc/group")).unwrap(), "root:x:0:\noperator:x:1000:\n");
        assert_eq!(fs::read(system.join("root/data")).unwrap(), b"source data");
        assert!(!system.join("root/.ssh").exists());
        let store = system.join(STORE);
        let mode = |path: &Path| fs::metadata(path).unwrap().mode() & 0o7777;
        assert_eq!(mode(&store), 0o700);
        assert_eq!(fs::read_to_string(store.join("etc/passwd")).unwrap(), "root:x:0:0:root:/root:/bin/sh\noperator:x:1000:1000:operator:/home/operator:/bin/bsh\n");
        assert_eq!(fs::read_to_string(store.join("etc/group")).unwrap(), "root:x:0:\noperator:x:1000:\n");
        assert_eq!(fs::read_to_string(store.join("etc/shadow")).unwrap(), "operator:!:::::::\n");
        assert_eq!(mode(&store.join("etc/shadow")), 0o600);
        let pending = store.join("var/lib/accounts/pending/operator");
        assert_eq!(fs::read_to_string(&pending).unwrap(), format!("{}\n", KEY));
        assert_eq!(mode(&pending), 0o600);
        // What the first generation's bsshd and the box keep there survives the next install.
        fs::create_dir(store.join("etc/ssh")).unwrap();
        fs::write(store.join("etc/ssh/ssh_host_ed25519_key"), b"host key").unwrap();
        fs::write(store.join("etc/shadow"), "operator:$6$set-on-the-box:::::::\n").unwrap();
        fs::remove_file(&pending).unwrap();
        // Upgrade an existing shell while keeping the password set on the machine.
        fs::write(store.join("etc/passwd"), "operator:x:1000:1000:operator:/home/operator:/bin/sh\n").unwrap();
        let second = fixture.source("second");
        fs::create_dir(second.join("etc/breenix")).unwrap();
        fs::write(second.join(OPERATOR), format!("user operator\nkey {}\n", KEY)).unwrap();
        install_tree(&second, &system, "second", &mut || Ok(())).unwrap();
        assert_eq!(fs::read_to_string(store.join("etc/passwd")).unwrap(), "operator:x:1000:1000:operator:/home/operator:/bin/bsh\n");
        assert_eq!(fs::read_to_string(store.join("etc/shadow")).unwrap(), "operator:$6$set-on-the-box:::::::\n");
        assert_eq!(fs::read(store.join("etc/ssh/ssh_host_ed25519_key")).unwrap(), b"host key");
        assert!(pending.exists());
        let tree = Tree::new(&system);
        for id in ["first", "second"] {
            let namespace = generation::select(&tree, id).unwrap();
            assert_eq!(tree.path(namespace.shared(namespace.root, STORE.as_bytes()).unwrap()), store);
        }
    }

    #[test]
    fn fixture_removal_keeps_machine_records_that_share_a_fixture_name() {
        let fixture = Fixture::new();
        let source = fixture.source("new");
        fs::create_dir_all(source.join("etc/breenix")).unwrap();
        let ua = "ua:x:2001:2001:Users suite fixture:/etc/breenix/test-users/ua:/bin/bsh";
        let uw = "uw:x:2003:2003:Users suite fixture:/etc/breenix/test-users/uw:/bin/bsh";
        fs::write(source.join("etc/passwd"), format!("root:x:0:0:root:/root:/bin/sh\n{ua}\n{uw}\n")).unwrap();
        fs::write(source.join("etc/group"), "root:x:0:\nwheel:x:10:uw\nua:x:2001:\nuw:x:2003:\n").unwrap();
        fs::write(source.join("etc/shadow"), "root:!:::::::\nua:!:::::::\nuw:!:::::::\n").unwrap();
        fs::write(source.join(FIXTURES), format!("passwd {ua}\npasswd {uw}\ngroup ua:x:2001:\ngroup uw:x:2003:\n\
            shadow ua:!:::::::\nshadow uw:!:::::::\nmember wheel uw\n")).unwrap();
        let factory = source.join("usr/share/factory/etc");
        fs::create_dir_all(&factory).unwrap();
        for file in ["passwd", "group", "shadow"] {
            fs::copy(source.join("etc").join(file), factory.join(file)).unwrap();
        }
        // The machine's own account named ua, in wheel, from before this install.
        let system = fixture.0.join("fresh");
        let store = system.join(STORE);
        fs::create_dir_all(store.join("etc")).unwrap();
        fs::write(store.join("etc/passwd"), "ua:x:1001:1001:machine account:/home/ua:/bin/bsh\n").unwrap();
        fs::write(store.join("etc/group"), "ua:x:1001:\nwheel:x:10:ua\n").unwrap();
        fs::write(store.join("etc/shadow"), "ua:$6$set-on-the-box:::::::\n").unwrap();
        install_tree(&source, &system, "first", &mut || Ok(())).unwrap();
        assert_eq!(fs::read_to_string(store.join("etc/passwd")).unwrap(),
            "root:x:0:0:root:/root:/bin/sh\nua:x:1001:1001:machine account:/home/ua:/bin/bsh\n");
        assert_eq!(fs::read_to_string(store.join("etc/group")).unwrap(), "root:x:0:\nwheel:x:10:ua\nua:x:1001:\n");
        assert_eq!(fs::read_to_string(store.join("etc/shadow")).unwrap(), "root:!:::::::\nua:$6$set-on-the-box:::::::\n");
        // The generation's factory defaults lose the fixtures, membership included.
        let factory = selected(&system, "first").join("usr/share/factory/etc");
        assert_eq!(fs::read_to_string(factory.join("passwd")).unwrap(), "root:x:0:0:root:/root:/bin/sh\n");
        assert_eq!(fs::read_to_string(factory.join("group")).unwrap(), "root:x:0:\nwheel:x:10:\n");
        assert_eq!(fs::read_to_string(factory.join("shadow")).unwrap(), "root:!:::::::\n");
    }

    #[test]
    fn operator_that_is_root_unparsable_or_colliding_is_refused_before_any_write() {
        for text in ["user root\nkey KEY\n".replace("KEY", KEY), "user operator\nkey -----BEGIN OPENSSH PRIVATE KEY-----\n".into(),
                     "user Operator\nkey KEY\n".replace("KEY", KEY), "user operator\n".into(),
                     "user operator\nkey KEY\nuser other\n".replace("KEY", KEY)] {
            let fixture = Fixture::new();
            let source = fixture.source("refused");
            fs::create_dir(source.join("etc/breenix")).unwrap();
            fs::write(source.join(OPERATOR), &text).unwrap();
            let system = fixture.system();
            assert!(install_tree(&source, &system, "new", &mut || Ok(())).is_err(), "{text:?}");
            assert!(!system.join(STORE).exists() && !system.join(GENERATIONS).exists(), "{text:?}");
        }
        let fixture = Fixture::new();
        let source = fixture.source("collide");
        fs::create_dir(source.join("etc/breenix")).unwrap();
        fs::write(source.join(OPERATOR), format!("user operator\nkey {}\n", KEY)).unwrap();
        let system = fixture.system();
        fs::create_dir_all(system.join(STORE).join("etc")).unwrap();
        fs::write(system.join(STORE).join("etc/passwd"), "someone:x:1000:1000::/home/someone:/bin/sh\n").unwrap();
        assert!(install_tree(&source, &system, "new", &mut || Ok(())).is_err());
        assert!(!system.join(GENERATIONS).exists());
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
        assert!(selected(&system, "old").join("etc").is_dir());
        assert!(selected(&system, "new").join("etc").is_dir());
        assert_eq!(fs::read(system.join(STORE).join("etc/config")).unwrap(), b"operator config");
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
        // These ownership equalities cover the host-accessible values only;
        // they do not prove chown preserves a distinct source uid or gid.
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
        let mut published = BTreeMap::new();
        for id in ["old", "new"] {
            let source = fixture.source(id);
            // Record the source before installation, independently of whatever
            // selection later returns. Compare ownership available to this host:
            // unprivileged tests cannot chown to arbitrary uid/gid values.
            recorded.insert(id, snapshot(&source));
            install_tree(&source, &system, id, &mut || Ok(())).unwrap();
            published.insert(id, snapshot(&selected(&system, id)));
        }
        let old = &recorded["old"];
        let new = &recorded["new"];
        let assert_contents = |root: &Path, expected: &BTreeMap<PathBuf, (u32, u32, u32, Vec<u8>, Option<u64>)>| {
            let actual = snapshot(root);
            let contents = |tree: &BTreeMap<PathBuf, (u32, u32, u32, Vec<u8>, Option<u64>)>| {
                tree.iter().filter(|(path, _)| {
                    !["root", "home", "dev", "proc", "sys", "etc", "var", "run", "tmp", "opt", "srv", READY].iter().any(|name| path.starts_with(name))
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
                    assert_eq!(fs::read(system.join(STORE).join("etc/config")).unwrap(), b"operator config");
                    let expected = if id == "old" { old } else { assert_eq!(id, "new"); new };
                    assert_contents(&selected_root, expected);
                    // Slot recovery must leave both complete trees unchanged,
                    // including mount placeholders, the record and every link.
                    for retained in ["old", "new"] {
                        assert_eq!(snapshot(&selected(&system, retained)), published[retained]);
                    }
                    assert_eq!(disk.files.borrow()["CURRENT.TXT"], id.as_bytes());
                    // Complete promotion then rollback without a fault: the
                    // actual slot transaction must return exactly the old tree.
                    if !rollback && id == "new" {
                        slots::begin(&disk, true).unwrap();
                        assert_eq!(disk.id("CURRENT"), "old");
                        assert_contents(&selected(&system, &disk.id("CURRENT")), old);
                        for retained in ["old", "new"] {
                            assert_eq!(snapshot(&selected(&system, retained)), published[retained]);
                        }
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
