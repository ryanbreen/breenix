//! The live `/etc` is the v2 shared binding of `/.breenix-shared/etc`.
//! Keep account updates in that binding rather than a second copy or a generation factory tree.
use super::parse;
#[cfg(not(target_vendor = "apple"))]
use super::PasswordLock;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, String>;
const MAX_DATABASE: u64 = 1024 * 1024;
static WRITER: Mutex<()> = Mutex::new(());
extern "C" {
    fn geteuid() -> u32;
    fn getuid() -> u32;
    fn getpid() -> i32;
    fn fchown(fd: i32, uid: u32, gid: u32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| e.to_string())
}
fn owner() -> u32 {
    // Scratch-root host tests use their own identity; installed Breenix files
    // belong to root, regardless of the invoking real identity.
    #[cfg(target_os = "breenix")]
    {
        0
    }
    #[cfg(not(target_os = "breenix"))]
    {
        unsafe { geteuid() }
    }
}
fn group_owner() -> u32 {
    #[cfg(target_os = "breenix")]
    {
        0
    }
    #[cfg(not(target_os = "breenix"))]
    {
        extern "C" {
            fn getegid() -> u32;
        }
        unsafe { getegid() }
    }
}

pub fn require_root() -> Result<()> {
    if unsafe { geteuid() } != 0 {
        return Err("root required".into());
    }
    Ok(())
}
pub fn today() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_div(86400)
        .min(u32::MAX as u64) as u32
}
fn name_ok(name: &str) -> Result<()> {
    if parse::valid_name(name.as_bytes()) {
        Ok(())
    } else {
        Err("invalid account name".into())
    }
}
fn checked(path: &Path, directory: bool, mode: u32) -> Result<fs::Metadata> {
    let m = io(fs::symlink_metadata(path))?;
    if m.uid() != owner()
        || m.gid() != group_owner()
        || m.mode() & 0o7777 != mode
        || (if directory {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1
        })
    {
        return Err(format!(
            "{}: unsafe kind, owner, link count or mode",
            path.display()
        ));
    }
    Ok(m)
}
fn owned_mode(file: &File, mode: u32) -> Result<()> {
    if unsafe { fchown(file.as_raw_fd(), owner(), group_owner()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    io(file.set_permissions(fs::Permissions::from_mode(mode)))
}
fn sync_dir(path: &Path) -> Result<()> {
    io(io(File::open(path))?.sync_all())
}

/// Held across the initial read, database publication and home operations.
/// POSIX locks are released by close or process exit; a crash leaves no stale lock.
pub struct Lock {
    file: File,
    _thread: MutexGuard<'static, ()>,
}
impl Lock {
    pub fn acquire(root: &Path) -> Result<Self> {
        let thread = WRITER.lock().map_err(|_| "account writer poisoned")?;
        checked(&root.join("etc"), true, 0o755)?;
        let path = root.join("etc/.pwd.lock");
        let file = io(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(&path))?;
        let before = checked(&path, false, 0o600)?;
        let opened = io(file.metadata())?;
        if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
            return Err("password lock changed during open".into());
        }
        lock_fd(file.as_raw_fd(), true)?;
        Ok(Self {
            file,
            _thread: thread,
        })
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = lock_fd(self.file.as_raw_fd(), false);
    }
}
fn lock_fd(fd: i32, lock: bool) -> Result<()> {
    #[cfg(not(target_vendor = "apple"))]
    let (mut record, command) = {
        let mut l = PasswordLock::write();
        if !lock {
            l.kind = 2;
        }
        (l, 7)
    };
    #[cfg(target_vendor = "apple")]
    let (mut record, command) = {
        // Darwin's host ABI differs from the installed Breenix ABI.
        #[repr(C)]
        struct HostLock {
            start: i64,
            len: i64,
            pid: i32,
            kind: i16,
            whence: i16,
        }
        (
            HostLock {
                start: 0,
                len: 0,
                pid: 0,
                kind: if lock { 3 } else { 2 },
                whence: 0,
            },
            9,
        )
    };
    loop {
        if unsafe { fcntl(fd, command, &mut record) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e.to_string());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub gecos: String,
    pub home: String,
    pub shell: String,
    pub groups: Vec<u32>,
}
fn text(b: &[u8]) -> String {
    std::str::from_utf8(b)
        .expect("parser validated UTF-8")
        .to_owned()
}

/// Raw lines are retained so unrelated malformed entries and local fields are
/// not discarded by an administrator changing a different account.
pub struct Database {
    pub passwd: String,
    pub group: String,
    pub shadow: String,
}
impl Database {
    pub fn read(root: &Path) -> Result<Self> {
        Ok(Self {
            passwd: read_file(root, "passwd", 0o644)?,
            group: read_file(root, "group", 0o644)?,
            shadow: read_file(root, "shadow", 0o600)?,
        })
    }
    pub fn account(&self, name: &str) -> Option<Account> {
        lookup(&self.passwd, &self.group, name)
    }
    pub fn shadow(&self, name: &str) -> Option<parse::Shadow<'_>> {
        let line = unique_line(&self.shadow, name)?;
        parse::Shadow::parse(line.as_bytes())
    }
    fn group_id(&self, name_or_id: &str) -> Result<u32> {
        let numeric = parse::id(name_or_id.as_bytes());
        self.group
            .lines()
            .filter_map(|l| parse::Group::parse(l.as_bytes()))
            .find(|g| g.fields[0] == name_or_id.as_bytes() || Some(g.gid) == numeric)
            .map(|g| g.gid)
            .ok_or_else(|| format!("unknown group {name_or_id}"))
    }
    fn available_name(&self, name: &str) -> Result<()> {
        name_ok(name)?;
        if self
            .passwd
            .lines()
            .chain(self.shadow.lines())
            .any(|l| line_name(l) == name)
        {
            Err("account already exists (including a disabled or incomplete entry)".into())
        } else {
            Ok(())
        }
    }
    fn free_uid(&self) -> Result<u32> {
        free_id(
            self.passwd
                .lines()
                .filter_map(|l| parse::Passwd::parse(l.as_bytes()).map(|p| p.uid)),
        )
    }
    fn free_gid(&self) -> Result<u32> {
        free_id(
            self.group
                .lines()
                .filter_map(|l| parse::Group::parse(l.as_bytes()).map(|g| g.gid)),
        )
    }
    /// Publish only changed files, shadow first. Checkpoints surround each
    /// filesystem operation for interrupted-write tests, not a production hook.
    pub fn publish(
        &self,
        root: &Path,
        original: &Self,
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        for (name, mode, data, old) in [
            ("shadow", 0o600, &self.shadow, &original.shadow),
            ("group", 0o644, &self.group, &original.group),
            ("passwd", 0o644, &self.passwd, &original.passwd),
        ] {
            if data != old {
                replace(root, name, data.as_bytes(), mode, checkpoint)?;
            }
        }
        Ok(())
    }
}
fn line_name(line: &str) -> &str {
    line.split(':').next().unwrap_or("")
}
fn unique_line<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let mut lines = text.lines().filter(|l| line_name(l) == name);
    let line = lines.next()?;
    if lines.next().is_some() {
        None
    } else {
        Some(line)
    }
}
pub fn lookup(passwd: &str, group: &str, name: &str) -> Option<Account> {
    if !parse::valid_name(name.as_bytes()) {
        return None;
    }
    let p = parse::Passwd::parse(unique_line(passwd, name)?.as_bytes())?;
    let mut groups = vec![p.gid];
    for g in group
        .lines()
        .filter_map(|l| parse::Group::parse(l.as_bytes()))
    {
        if g.fields[3]
            .split(|b| *b == b',')
            .any(|m| m == name.as_bytes())
            && !groups.contains(&g.gid)
        {
            groups.push(g.gid);
        }
    }
    Some(Account {
        name: name.into(),
        uid: p.uid,
        gid: p.gid,
        gecos: text(p.fields[4]),
        home: text(p.fields[5]),
        shell: text(p.fields[6]),
        groups,
    })
}
fn free_id(ids: impl Iterator<Item = u32>) -> Result<u32> {
    let used: std::collections::BTreeSet<_> = ids.collect();
    (1000..u32::MAX)
        .find(|id| !used.contains(id))
        .ok_or_else(|| "no free IDs".into())
}
fn read_file(root: &Path, name: &str, mode: u32) -> Result<String> {
    let path = root.join("etc").join(name);
    let m = checked(&path, false, mode)?;
    if m.len() > MAX_DATABASE {
        return Err("account database too large".into());
    }
    let file = io(File::open(&path))?;
    let opened = io(file.metadata())?;
    if (m.dev(), m.ino()) != (opened.dev(), opened.ino()) {
        return Err("account database changed during open".into());
    }
    let mut out = String::new();
    io(file.take(MAX_DATABASE + 1).read_to_string(&mut out))?;
    if out.len() as u64 > MAX_DATABASE {
        return Err("account database too large".into());
    }
    Ok(out)
}
/// Write `<file>+` with final metadata, fsync, rename, then fsync `/etc`.
/// The caller must hold Lock until this and its other publications finish.
pub fn replace(
    root: &Path,
    name: &str,
    bytes: &[u8],
    mode: u32,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    if !matches!(
        (name, mode),
        ("passwd", 0o644) | ("group", 0o644) | ("shadow", 0o600)
    ) {
        return Err("invalid account file".into());
    }
    let etc = root.join("etc");
    let scratch = etc.join(format!("{name}+"));
    let step = |op: &mut dyn FnMut() -> Result<()>,
                action: &mut dyn FnMut() -> Result<()>|
     -> Result<()> {
        op()?;
        action()?;
        op()
    };
    if io(scratch.try_exists())? || fs::symlink_metadata(&scratch).is_ok() {
        // Stale scratch belongs to the previous lock holder. Unlink it rather than
        // following a link or truncating a possibly aliased inode.
        step(checkpoint, &mut || io(fs::remove_file(&scratch)))?;
    }
    checkpoint()?;
    let mut f = io(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&scratch))?;
    checkpoint()?;
    step(checkpoint, &mut || owned_mode(&f, mode))?;
    step(checkpoint, &mut || io(f.write_all(bytes)))?;
    step(checkpoint, &mut || io(f.sync_all()))?;
    drop(f);
    step(checkpoint, &mut || io(fs::rename(&scratch, etc.join(name))))?;
    step(checkpoint, &mut || sync_dir(&etc))
}
fn append_line(text: &mut String, line: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
}
fn edit(text: &mut String, name: &str, replacement: Option<&str>) {
    let mut out = String::new();
    for line in text.lines() {
        if line_name(line) != name {
            append_line(&mut out, line);
        } else if let Some(replacement) = replacement {
            append_line(&mut out, replacement);
        }
    }
    *text = out;
}
fn transaction<T>(root: &Path, action: impl FnOnce(&mut Database) -> Result<T>) -> Result<T> {
    let _lock = Lock::acquire(root)?;
    let original = Database::read(root)?;
    let mut changed = Database {
        passwd: original.passwd.clone(),
        group: original.group.clone(),
        shadow: original.shadow.clone(),
    };
    let result = action(&mut changed)?;
    changed.publish(root, &original, &mut || Ok(()))?;
    Ok(result)
}

#[derive(Default)]
pub struct UserAdd {
    pub uid: Option<u32>,
    pub group: Option<String>,
    pub groups: Vec<String>,
    pub shell: Option<String>,
    pub home: Option<String>,
    pub create_home: bool,
}
/// Add a locked user and, without -g, a same-named primary group.
pub fn useradd(root: &Path, name: &str, options: &UserAdd) -> Result<Account> {
    require_root()?;
    transaction(root, |db| {
        db.available_name(name)?;
        let uid = options.uid.unwrap_or(db.free_uid()?);
        if uid == u32::MAX
            || db
                .passwd
                .lines()
                .filter_map(|l| parse::Passwd::parse(l.as_bytes()))
                .any(|p| p.uid == uid)
        {
            return Err("user ID already exists or is invalid".into());
        }
        let gid = if let Some(g) = &options.group {
            db.group_id(g)?
        } else {
            if db.group.lines().any(|l| line_name(l) == name) {
                return Err("same-named group already exists; select it with -g".into());
            }
            let gid = if db
                .group
                .lines()
                .filter_map(|l| parse::Group::parse(l.as_bytes()))
                .any(|g| g.gid == uid)
            {
                db.free_gid()?
            } else {
                uid
            };
            append_line(&mut db.group, &format!("{name}:x:{gid}:"));
            gid
        };
        let shell = options.shell.as_deref().unwrap_or("/bin/bsh");
        let home = options
            .home
            .clone()
            .unwrap_or_else(|| format!("/home/{name}"));
        if !parse::absolute(shell.as_bytes())
            || !parse::absolute(home.as_bytes())
            || shell.contains(':')
            || home.contains(':')
            || shell
                .bytes()
                .chain(home.bytes())
                .any(|b| b.is_ascii_control())
        {
            return Err("invalid home or shell".into());
        }
        append_line(
            &mut db.shadow,
            &format!("{name}:!:{}:0:99999:7:::", today()),
        );
        append_line(
            &mut db.passwd,
            &format!("{name}:x:{uid}:{gid}::{home}:{shell}"),
        );
        membership(db, name, &options.groups, false)?;
        let account = db.account(name).ok_or("new account did not parse")?;
        if options.create_home {
            make_home(root, &account)?;
        }
        Ok(account)
    })
}
fn membership(db: &mut Database, name: &str, groups: &[String], append: bool) -> Result<()> {
    let ids: Vec<_> = groups
        .iter()
        .map(|g| db.group_id(g))
        .collect::<Result<_>>()?;
    let mut out = String::new();
    for line in db.group.lines() {
        if let Some(g) = parse::Group::parse(line.as_bytes()) {
            let mut members: Vec<_> = text(g.fields[3])
                .split(',')
                .filter(|m| !m.is_empty())
                .map(str::to_owned)
                .collect();
            if !append {
                members.retain(|m| m != name);
            }
            if ids.contains(&g.gid) && !members.iter().any(|m| m == name) {
                members.push(name.into());
            }
            if members.join(",").as_bytes() == g.fields[3] {
                append_line(&mut out, line);
            } else {
                append_line(
                    &mut out,
                    &format!("{}:x:{}:{}", text(g.fields[0]), g.gid, members.join(",")),
                );
            }
        } else {
            append_line(&mut out, line);
        }
    }
    db.group = out;
    Ok(())
}

#[derive(Default)]
pub struct UserMod {
    pub groups: Option<Vec<String>>,
    pub append: bool,
    pub shell: Option<String>,
    pub lock: bool,
    pub unlock: bool,
    pub expire: Option<Option<u32>>,
}
pub fn usermod(root: &Path, name: &str, options: &UserMod) -> Result<()> {
    require_root()?;
    transaction(root, |db| {
        let mut a = db.account(name).ok_or("unknown or malformed account")?;
        if options.append && options.groups.is_none() || options.lock && options.unlock {
            return Err("incompatible usermod options".into());
        }
        if let Some(groups) = &options.groups {
            membership(db, name, groups, options.append)?;
        }
        if let Some(shell) = &options.shell {
            if !parse::absolute(shell.as_bytes())
                || shell.contains(':')
                || shell.bytes().any(|b| b.is_ascii_control())
            {
                return Err("invalid shell".into());
            }
            a.shell = shell.clone();
            edit(
                &mut db.passwd,
                name,
                Some(&format!(
                    "{name}:x:{}:{}:{}:{}:{}",
                    a.uid, a.gid, a.gecos, a.home, a.shell
                )),
            );
        }
        if options.lock || options.unlock || options.expire.is_some() {
            let s = db.shadow(name).ok_or("missing or malformed shadow entry")?;
            let mut f: Vec<String> = s.fields.iter().map(|v| text(v)).collect();
            if options.lock && !f[1].starts_with('!') {
                f[1].insert(0, '!');
            }
            if options.unlock {
                let hash = f[1].strip_prefix('!').ok_or("password is not locked")?;
                if !hash.starts_with("$6$") {
                    return Err("unlock would leave no usable password".into());
                }
                f[1] = hash.into();
            }
            if let Some(expire) = options.expire {
                f[7] = expire.map(|d| d.to_string()).unwrap_or_default();
            }
            edit(&mut db.shadow, name, Some(&f.join(":")));
        }
        Ok(())
    })
}
pub fn groupadd(root: &Path, name: &str, gid: Option<u32>) -> Result<()> {
    require_root()?;
    transaction(root, |db| {
        name_ok(name)?;
        if db.group.lines().any(|l| line_name(l) == name) {
            return Err("group already exists".into());
        }
        let gid = gid.unwrap_or(db.free_gid()?);
        if gid == u32::MAX
            || db
                .group
                .lines()
                .filter_map(|l| parse::Group::parse(l.as_bytes()))
                .any(|g| g.gid == gid)
        {
            return Err("group ID already exists or is invalid".into());
        }
        append_line(&mut db.group, &format!("{name}:x:{gid}:"));
        Ok(())
    })
}
pub fn groupdel(root: &Path, name: &str) -> Result<()> {
    require_root()?;
    transaction(root, |db| {
        let gid = db.group_id(name)?;
        if db
            .passwd
            .lines()
            .filter_map(|l| parse::Passwd::parse(l.as_bytes()))
            .any(|p| p.gid == gid)
        {
            return Err("group is an account's primary group".into());
        }
        edit(&mut db.group, name, None);
        Ok(())
    })
}
pub fn userdel(root: &Path, name: &str, remove_home: bool) -> Result<()> {
    require_root()?;
    transaction(root, |db| {
        let a = db.account(name).ok_or("unknown or malformed account")?;
        if a.uid < 1000 {
            return Err("refusing removal of a system account".into());
        }
        if user_has_process(root, a.uid)? {
            return Err("account has running processes".into());
        }
        if remove_home {
            remove_owned_home(root, &a)?;
        }
        edit(&mut db.passwd, name, None);
        edit(&mut db.shadow, name, None);
        membership(db, name, &[], false)?;
        // Remove only an unused, empty private group with this exact name.
        if let Some(line) = unique_line(&db.group, name) {
            if let Some(g) = parse::Group::parse(line.as_bytes()) {
                if g.gid == a.gid
                    && g.fields[3].is_empty()
                    && !db
                        .passwd
                        .lines()
                        .filter_map(|l| parse::Passwd::parse(l.as_bytes()))
                        .any(|p| p.gid == a.gid)
                {
                    edit(&mut db.group, name, None);
                }
            }
        }
        Ok(())
    })
}
fn user_has_process(root: &Path, uid: u32) -> Result<bool> {
    for e in io(fs::read_dir(root.join("proc")))? {
        let e = io(e)?;
        if !e
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let status = match fs::read_to_string(e.path().join("status")) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("cannot check processes: {e}")),
        };
        let ids = status
            .lines()
            .find_map(|l| l.strip_prefix("Uid:"))
            .ok_or("process status has no user IDs")?;
        if ids
            .split_whitespace()
            .filter_map(|s| s.parse::<u32>().ok())
            .any(|id| id == uid)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn home_path(root: &Path, home: &str) -> Result<PathBuf> {
    if !parse::absolute(home.as_bytes()) {
        return Err("invalid home".into());
    }
    let path = root.join(home.trim_start_matches('/'));
    // Require real, root-owned parent directories that the account cannot
    // rename or steer through a symlink. This permits -d /srv/name as well as
    // the default /home/name without handing recursive removal an alias.
    let parent = path.parent().ok_or("home has no parent")?;
    let mut current = root.to_path_buf();
    for part in parent
        .strip_prefix(root)
        .map_err(|_| "home outside root")?
        .components()
    {
        current.push(part);
        let m = io(fs::symlink_metadata(&current))?;
        if !m.is_dir() || m.uid() != owner() || m.mode() & 0o022 != 0 {
            return Err("unsafe home parent".into());
        }
    }
    Ok(path)
}
fn make_home(root: &Path, a: &Account) -> Result<()> {
    let path = home_path(root, &a.home)?;
    io(fs::create_dir(&path))?;
    io(fs::set_permissions(
        &path,
        fs::Permissions::from_mode(0o700),
    ))?;
    // Build while root owns the new 0700 directory. Ownership is handed over
    // after the skeleton copy finishes without following links.
    copy_skel(&root.join("etc/skel"), &path, a.uid, a.gid)?;
    io(std::os::unix::fs::chown(&path, Some(a.uid), Some(a.gid)))?;
    sync_dir(&path)?;
    sync_dir(path.parent().ok_or("home has no parent")?)
}
fn copy_skel(from: &Path, to: &Path, uid: u32, gid: u32) -> Result<()> {
    let m = io(fs::symlink_metadata(from))?;
    if !m.is_dir() || m.uid() != owner() || m.mode() & 0o022 != 0 {
        return Err("unsafe skeleton directory".into());
    }
    for e in io(fs::read_dir(from))? {
        let e = io(e)?;
        let src = e.path();
        let dest = to.join(e.file_name());
        let m = io(fs::symlink_metadata(&src))?;
        if m.uid() != owner() || m.mode() & 0o022 != 0 {
            return Err("unsafe skeleton entry".into());
        }
        if m.is_dir() {
            io(fs::create_dir(&dest))?;
            copy_skel(&src, &dest, uid, gid)?;
        } else if m.is_file() {
            let mut source = io(File::open(&src))?;
            let mut f = io(OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&dest))?;
            io(std::io::copy(&mut source, &mut f))?;
            io(f.sync_all())?;
        } else {
            return Err("skeleton links and special files are refused".into());
        }
        io(std::os::unix::fs::chown(&dest, Some(uid), Some(gid)))?;
        io(fs::set_permissions(
            &dest,
            fs::Permissions::from_mode(m.mode() & 0o777),
        ))?;
        if m.is_dir() {
            sync_dir(&dest)?;
        } else {
            io(io(File::open(&dest))?.sync_all())?;
        }
    }
    Ok(())
}
fn remove_owned_home(root: &Path, a: &Account) -> Result<()> {
    let path = home_path(root, &a.home)?;
    let m = io(fs::symlink_metadata(&path))?;
    if !m.is_dir() || m.uid() != a.uid {
        return Err("home is not a directory owned by the account".into());
    }
    // Freeze the directory against the deleted user before traversing it.
    io(std::os::unix::fs::chown(
        &path,
        Some(owner()),
        Some(group_owner()),
    ))?;
    io(fs::set_permissions(
        &path,
        fs::Permissions::from_mode(0o700),
    ))?;
    io(fs::remove_dir_all(&path))?;
    sync_dir(path.parent().ok_or("home has no parent")?)
}

/// Set a password through the privileged side of passwd. Caller identity is
/// read from the kernel rather than a caller argument. The old password is
/// checked while the same lock protects the replacement.
pub fn change_password(root: &Path, name: &str, old: &[u8], new: &[u8]) -> Result<()> {
    require_root()?;
    if new.len() < 8 || new.len() > 1024 || new.contains(&0) {
        return Err("password must contain 8 to 1024 bytes".into());
    }
    let changed = transaction(root, |db| {
        let a = db.account(name).ok_or("unknown or malformed account")?;
        let s = db.shadow(name).ok_or("missing or malformed shadow entry")?;
        let caller = unsafe { getuid() };
        if caller != 0 {
            if caller != a.uid {
                return Err("may change only your own password".into());
            }
            if !s.fields[1].starts_with(b"$6$")
                || !crate::crypto::sha512_crypt::verify(old, &text(s.fields[1]))
            {
                return Err("old password refused".into());
            }
            if s.last_change
                .zip(s.minimum)
                .is_some_and(|(last, min)| today() < last.saturating_add(min))
            {
                return Err("minimum password age not reached".into());
            }
        }
        let mut random = [0; 16];
        random_bytes(&mut random)?;
        let alphabet = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let salt: String = random
            .iter()
            .map(|b| alphabet[(b & 63) as usize] as char)
            .collect();
        let mut out = [0; crate::crypto::sha512_crypt::MAX_OUTPUT];
        let n = crate::crypto::sha512_crypt::sha512_crypt(
            new,
            &format!("$6$rounds=100000${salt}"),
            &mut out,
        )?;
        let mut f: Vec<String> = s.fields.iter().map(|v| text(v)).collect();
        f[1] = text(&out[..n]);
        f[2] = today().to_string();
        edit(&mut db.shadow, name, Some(&f.join(":")));
        Ok(())
    });
    let logged = audit(root, "passwd", name, changed.is_ok());
    changed.and(logged)
}
fn random_bytes(bytes: &mut [u8]) -> Result<()> {
    #[cfg(target_os = "breenix")]
    {
        let mut at = 0;
        while at < bytes.len() {
            let n = crate::crypto::rand::getrandom_bytes(&mut bytes[at..])
                .map_err(|e| format!("getrandom: {e}"))?;
            if n == 0 {
                return Err("getrandom returned no bytes".into());
            }
            at += n;
        }
        Ok(())
    }
    #[cfg(not(target_os = "breenix"))]
    {
        io(io(File::open("/dev/urandom"))?.read_exact(bytes))
    }
}

/// Append a bounded audit record, omitting passwords and hashes. Site-specific
/// target names are runtime data only. The file is created by layout at 0640.
pub fn audit(root: &Path, program: &str, target: &str, success: bool) -> Result<()> {
    let path = root.join("var/log/auth.log");
    let m = io(fs::symlink_metadata(&path))?;
    if !m.is_file() || m.uid() != owner() || m.mode() & 0o7777 != 0o640 {
        return Err("unsafe auth log".into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    // Gregorian civil date from elapsed days, using 400-year eras.
    let z = days as i64 + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(month <= 2);
    let safe = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric() || "_-/".contains(*c))
            .take(64)
            .collect()
    };
    let line = format!("{y:04}-{month:02}-{d:02}T{:02}:{:02}:{:02}Z {}[{}] user={} target={} tty=- result={} cmd=\n",
        now%86400/3600, now%3600/60, now%60, safe(program), unsafe { getpid() }, unsafe { getuid() }, safe(target), if success { "ok" } else { "refused:operation" });
    let mut f = io(OpenOptions::new().append(true).open(&path))?;
    io(f.write_all(line.as_bytes()))?;
    io(f.sync_all())?;
    let mut kmsg = io(OpenOptions::new().write(true).open(root.join("dev/kmsg")))?;
    io(kmsg.write_all(line.as_bytes()))
}

/// Add required factory system entries through the same lock and publication
/// protocol as the tools. Existing lines, including disabled ones, win.
pub fn reconcile_system(root: &Path, checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<()> {
    let _lock = Lock::acquire(root)?;
    // Shadow first for new entries, just as useradd; this also seeds an absent
    // database before the general factory reconciler handles ordinary defaults.
    for (name, mode) in [("shadow", 0o600), ("group", 0o644), ("passwd", 0o644)] {
        let factory = root.join("usr/share/factory/etc").join(name);
        if !factory.exists() {
            continue;
        }
        let incoming = io(fs::read_to_string(&factory))?;
        let path = root.join("etc").join(name);
        let present = match fs::symlink_metadata(&path) {
            Ok(_) => Some(read_file(root, name, mode)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        let mut merged = present.clone().unwrap_or_default();
        for line in incoming.lines() {
            let valid = match name {
                "passwd" => parse::Passwd::parse(line.as_bytes()).is_some(),
                "group" => parse::Group::parse(line.as_bytes()).is_some(),
                _ => parse::Shadow::parse(line.as_bytes()).is_some(),
            };
            if !valid {
                return Err(format!("invalid factory {name} entry"));
            }
            if !merged.lines().any(|old| line_name(old) == line_name(line)) {
                append_line(&mut merged, line);
            }
        }
        if present.as_deref() != Some(&merged) {
            replace(root, name, merged.as_bytes(), mode, checkpoint)?;
        }
    }
    Ok(())
}
