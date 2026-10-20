//! Accounts for SSH logins.
//!
//! - The account database: `/etc/passwd` and `/etc/group` (POSIX `getpwnam`,
//!   `getgrouplist` semantics, read directly). bsshd refuses root (uid 0); it is
//!   administered from an account that logs in.
//! - Authorized keys: `~/.ssh/authorized_keys` of the account, used only when
//!   the file, `~/.ssh`, the home directory and each directory above it are
//!   owned by root or the account, writable by no one else, and not symbolic
//!   links.
//! - The session's credentials: supplementary groups, gid and uid of the
//!   account, set in that order and then checked, as `login` does.
//!
//! Each path is taken under a `root` directory, `/` in bsshd, so the host
//! tests can run the same code on a scratch tree.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::auth::Authorizer;
use super::keys;

/// The largest authorized_keys file read.
const MAX_AUTHORIZED_KEYS: u64 = 64 * 1024;

/// One account from the passwd database, with its groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
    /// The primary group first, then each group that lists the account.
    pub groups: Vec<u32>,
}

/// Why a login or a file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

fn refuse<T>(text: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(text.into()))
}

/// A user name bsshd will look up: a letter, digit or `_` first, then up to 31
/// of letters, digits, `.`, `_` and `-` (the POSIX portable user name set).
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && (bytes[0].is_ascii_alphanumeric() || bytes[0] == b'_')
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(b))
}

/// The account `name` from passwd and group text; an unknown or malformed
/// entry gives no account.
pub fn lookup(passwd: &str, group: &str, name: &str) -> Option<Account> {
    if !valid_name(name) {
        return None;
    }
    let fields: Vec<&str> = passwd
        .lines()
        .map(|line| line.split(':').collect::<Vec<&str>>())
        .find(|fields| fields.first() == Some(&name))?;
    if fields.len() != 7 || !fields[5].starts_with('/') || !fields[6].starts_with('/') {
        return None;
    }
    let uid: u32 = fields[2].parse().ok()?;
    let gid: u32 = fields[3].parse().ok()?;
    let mut groups = vec![gid];
    for line in group.lines() {
        let entry: Vec<&str> = line.split(':').collect();
        if entry.len() != 4 {
            continue;
        }
        let Ok(id) = entry[2].parse::<u32>() else { continue };
        if entry[3].split(',').any(|member| member == name) && !groups.contains(&id) {
            groups.push(id);
        }
    }
    Some(Account {
        name: name.to_string(),
        uid,
        gid,
        home: fields[5].to_string(),
        shell: fields[6].to_string(),
        groups,
    })
}

/// Read the account `name` from `root`'s passwd and group files.
pub fn read_account(root: &Path, name: &str) -> Result<Account, Refusal> {
    let passwd = fs::read_to_string(root.join("etc/passwd")).or_else(|e| refuse(format!("/etc/passwd: {}", e)))?;
    let group = fs::read_to_string(root.join("etc/group")).unwrap_or_default();
    lookup(&passwd, &group, name).ok_or_else(|| Refusal(format!("no account '{}'", name)))
}

/// Whether the account may log in over SSH at all: any account but root's.
pub fn may_log_in(account: &Account) -> Result<(), Refusal> {
    if account.uid == 0 {
        return refuse(format!("'{}' has uid 0, and root does not log in over SSH", account.name));
    }
    Ok(())
}

pub(crate) fn under(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}

extern "C" {
    fn geteuid() -> u32;
}

/// Root and the account: the owners an account's own files may have.
fn account_owners(account: &Account) -> [u32; 2] {
    [0, account.uid]
}

/// Root and this process (root itself in bsshd and init): the owners of files
/// that only the system may write.
pub(crate) fn system_owners() -> [u32; 2] {
    [0, unsafe { geteuid() }]
}

/// Refuse anything but a `directory` (or a regular file) owned by one of
/// `owners` and writable by neither group nor others.
pub(crate) fn check_owned(path: &Path, owners: [u32; 2], directory: bool) -> Result<fs::Metadata, Refusal> {
    let meta = fs::symlink_metadata(path).or_else(|e| refuse(format!("{}: {}", path.display(), e)))?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        return refuse(format!("{} is a symbolic link", path.display()));
    }
    if directory && !kind.is_dir() || !directory && !kind.is_file() {
        return refuse(format!("{} is not a {}", path.display(), if directory { "directory" } else { "regular file" }));
    }
    if !owners.contains(&meta.uid()) {
        return refuse(format!("{} is owned by uid {}", path.display(), meta.uid()));
    }
    if meta.mode() & 0o022 != 0 {
        return refuse(format!("{} is writable by group or others (mode {:o})", path.display(), meta.mode() & 0o7777));
    }
    Ok(meta)
}

/// The directories from `root` down to the account's home, then `~/.ssh`.
fn checked_directories(root: &Path, account: &Account) -> Result<PathBuf, Refusal> {
    let mut path = root.to_path_buf();
    check_owned(&path, account_owners(account), true)?;
    for component in account.home.split('/').filter(|part| !part.is_empty()) {
        if component == "." || component == ".." {
            return refuse(format!("home {} is not a plain path", account.home));
        }
        path.push(component);
        check_owned(&path, account_owners(account), true)?;
    }
    path.push(".ssh");
    check_owned(&path, account_owners(account), true)?;
    Ok(path)
}

/// Open `path`, a regular file owned by one of `owners`, refusing a file that
/// changed between the check and the open.
pub(crate) fn open_checked(path: &Path, owners: [u32; 2], append: bool) -> Result<File, Refusal> {
    let checked = check_owned(path, owners, false)?;
    let file = OpenOptions::new()
        .read(!append)
        .append(append)
        .open(path)
        .or_else(|e| refuse(format!("{}: {}", path.display(), e)))?;
    let opened = file.metadata().or_else(|e| refuse(format!("{}: {}", path.display(), e)))?;
    if opened.dev() != checked.dev() || opened.ino() != checked.ino() {
        return refuse(format!("{} changed while it was opened", path.display()));
    }
    Ok(file)
}

/// The public-key blobs of the account's authorized_keys, after the ownership
/// and mode checks. Lines that are not `type base64 [comment]` for a supported
/// key type are ignored: key options are not implemented, so a key restricted
/// by options is not accepted at all.
pub fn authorized_keys(root: &Path, account: &Account) -> Result<Vec<Vec<u8>>, Refusal> {
    let path = checked_directories(root, account)?.join("authorized_keys");
    let mut file = open_checked(&path, account_owners(account), false)?;
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(MAX_AUTHORIZED_KEYS + 1)
        .read_to_string(&mut text)
        .or_else(|e| refuse(format!("{}: {}", path.display(), e)))?;
    if text.len() as u64 > MAX_AUTHORIZED_KEYS {
        return refuse(format!("{} is larger than {} bytes", path.display(), MAX_AUTHORIZED_KEYS));
    }
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(keys::parse_public_line)
        .collect())
}

/// The authorizer bsshd uses: the account must exist, must not be root's, and
/// its authorized_keys, read under `root`, must hold the key.
pub struct FileAuthorizer {
    root: PathBuf,
}

impl FileAuthorizer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Authorizer for FileAuthorizer {
    fn authorized(&mut self, user: &str, key_blob: &[u8]) -> bool {
        let account = match read_account(&self.root, user).and_then(|account| may_log_in(&account).map(|_| account)) {
            Ok(account) => account,
            Err(why) => {
                println!("bsshd: login refused: {}", why);
                return false;
            }
        };
        match authorized_keys(&self.root, &account) {
            Ok(blobs) => blobs.iter().any(|blob| keys::ct_eq(blob, key_blob)),
            Err(why) => {
                println!("bsshd: authorized keys of '{}' refused: {}", user, why);
                false
            }
        }
    }
}

// ==========================================================================
// Adding a key
// ==========================================================================

fn sync_directory(path: &Path) -> Result<(), Refusal> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .or_else(|e| refuse(format!("sync {}: {}", path.display(), e)))
}

/// Create `path` as a directory with `mode`, owned by the account, unless it
/// exists; an existing one must pass the ownership checks.
fn ensure_directory(path: &Path, account: &Account, mode: u32) -> Result<(), Refusal> {
    match fs::symlink_metadata(path) {
        Ok(_) => check_owned(path, account_owners(account), true).map(|_| ()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).or_else(|e| refuse(format!("mkdir {}: {}", path.display(), e)))?;
            std::os::unix::fs::chown(path, Some(account.uid), Some(account.gid))
                .or_else(|e| refuse(format!("chown {}: {}", path.display(), e)))?;
            fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode))
                .or_else(|e| refuse(format!("chmod {}: {}", path.display(), e)))?;
            sync_directory(path.parent().unwrap_or(Path::new("/")))
        }
        Err(e) => refuse(format!("{}: {}", path.display(), e)),
    }
}

/// Add `line` (for the key `blob`) to the account's authorized_keys unless it
/// is already there, creating the home directory, `~/.ssh` and the file as
/// needed. Returns whether a line was added.
pub fn authorize_key(root: &Path, account: &Account, blob: &[u8], line: &str) -> Result<bool, Refusal> {
    let home = under(root, &account.home);
    let parent = home.parent().ok_or_else(|| Refusal(format!("home {} has no parent", account.home)))?;
    // The directories above the home are the system's: an account cannot
    // steer where its home is created.
    let mut above = root.to_path_buf();
    check_owned(&above, system_owners(), true)?;
    for component in parent.strip_prefix(root).unwrap_or(parent).components() {
        above.push(component);
        check_owned(&above, system_owners(), true)?;
    }
    ensure_directory(&home, account, 0o700)?;
    let ssh = home.join(".ssh");
    ensure_directory(&ssh, account, 0o700)?;
    let path = ssh.join("authorized_keys");
    let present = fs::symlink_metadata(&path).is_ok();
    if present && authorized_keys(root, account)?.iter().any(|known| keys::ct_eq(known, blob)) {
        return Ok(false);
    }
    let mut file = if present {
        open_checked(&path, account_owners(account), true)?
    } else {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .or_else(|e| refuse(format!("create {}: {}", path.display(), e)))?;
        std::os::unix::fs::chown(&path, Some(account.uid), Some(account.gid))
            .or_else(|e| refuse(format!("chown {}: {}", path.display(), e)))?;
        file
    };
    file.write_all(format!("{}\n", line).as_bytes())
        .and_then(|_| file.sync_all())
        .or_else(|e| refuse(format!("write {}: {}", path.display(), e)))?;
    sync_directory(&ssh)?;
    Ok(true)
}

// ==========================================================================
// The session's credentials
// ==========================================================================

/// The process credential calls a session makes; bsshd's are the system calls.
pub trait Credentials {
    fn set_groups(&mut self, groups: &[u32]) -> Result<(), i32>;
    fn set_gid(&mut self, gid: u32) -> Result<(), i32>;
    fn set_uid(&mut self, uid: u32) -> Result<(), i32>;
    /// Real uid, effective uid, real gid and effective gid.
    fn ids(&self) -> (u32, u32, u32, u32);
    fn groups(&self) -> Vec<u32>;
}

/// Take on the account's supplementary groups, gid and uid, in that order (the
/// group calls need privilege that setuid gives up), then confirm the result.
/// For any account but root, also confirm that root cannot be regained. Any
/// failure is an error, and the caller must not start the session.
pub fn become_account(account: &Account, credentials: &mut dyn Credentials) -> Result<(), Refusal> {
    credentials.set_groups(&account.groups).or_else(|e| refuse(format!("setgroups: errno {}", e)))?;
    credentials.set_gid(account.gid).or_else(|e| refuse(format!("setgid({}): errno {}", account.gid, e)))?;
    credentials.set_uid(account.uid).or_else(|e| refuse(format!("setuid({}): errno {}", account.uid, e)))?;
    let ids = credentials.ids();
    if ids != (account.uid, account.uid, account.gid, account.gid) {
        return refuse(format!("credentials are {:?} after the switch to {}", ids, account.name));
    }
    let mut held = credentials.groups();
    held.sort_unstable();
    held.dedup();
    let mut wanted = account.groups.clone();
    wanted.sort_unstable();
    if held != wanted {
        return refuse(format!("groups are {:?} after the switch to {}", held, account.name));
    }
    if account.uid != 0 && credentials.set_uid(0).is_ok() {
        return refuse(format!("{} could set uid 0 again", account.name));
    }
    Ok(())
}

/// The session's environment, replacing bsshd's own.
pub fn login_environment(account: &Account, term: Option<&str>) -> Vec<String> {
    let path = if account.uid == 0 { "/bin:/sbin:/usr/local/bin:/usr/local/cbin" } else { "/bin:/usr/local/bin:/usr/local/cbin" };
    let mut environment = vec![
        format!("HOME={}", account.home),
        format!("USER={}", account.name),
        format!("LOGNAME={}", account.name),
        format!("SHELL={}", account.shell),
        format!("PATH={}", path),
    ];
    if let Some(term) = term.filter(|term| !term.is_empty() && term.bytes().all(|b| b.is_ascii_graphic())) {
        environment.push(format!("TERM={}", term));
    }
    environment
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const OPERATOR: &str = "operator";
    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bsh\n\
        nobody:x:65534:65534:nobody:/nonexistent:/bin/false\n\
        operator:x:1000:1000:operator:/home/operator:/bin/bsh\n\
        broken:x:12:not-a-number:broken:/home/broken:/bin/bsh\n";
    const GROUP: &str = "root:x:0:\nnobody:x:65534:\noperator:x:1000:\nwheel:x:10:root,operator\nmedia:x:20:someone\n";

    extern "C" {
        fn getuid() -> u32;
        fn getgid() -> u32;
    }

    /// A scratch root holding passwd and group, and the test user standing in
    /// for the account (files cannot be given to another uid without privilege).
    struct Tree {
        root: PathBuf,
        account: Account,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("bssh-account-{}-{}", name, std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("etc/breenix")).unwrap();
            let (uid, gid) = unsafe { (getuid(), getgid()) };
            let passwd = PASSWD.replace(":1000:1000:", &format!(":{}:{}:", uid, gid));
            fs::write(root.join("etc/passwd"), passwd).unwrap();
            fs::write(root.join("etc/group"), GROUP).unwrap();
            for dir in [root.clone(), root.join("etc"), root.join("etc/breenix")] {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let account = read_account(&root, OPERATOR).unwrap();
            Tree { root, account }
        }

        fn home(&self) -> PathBuf {
            under(&self.root, &self.account.home)
        }

        fn authorize(&self, line: &str) {
            let ssh = self.home().join(".ssh");
            fs::create_dir_all(&ssh).unwrap();
            for dir in [self.root.join("home"), self.home(), ssh.clone()] {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
            }
            fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
            let file = ssh.join("authorized_keys");
            fs::write(&file, format!("# operator keys\n{}\n", line)).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn key(seed: u8) -> keys::Ed25519Key {
        keys::Ed25519Key::from_seed(&[seed; 32])
    }

    #[test]
    fn accounts_come_from_passwd_with_their_groups() {
        let operator = lookup(PASSWD, GROUP, "operator").unwrap();
        assert_eq!((operator.uid, operator.gid, operator.home.as_str()), (1000, 1000, "/home/operator"));
        assert_eq!(operator.groups, vec![1000, 10]);
        assert_eq!(lookup(PASSWD, GROUP, "root").unwrap().groups, vec![0, 10]);
        for unknown in ["admin", "", "broken", "root:x", "../root", "-root", "oper ator"] {
            assert_eq!(lookup(PASSWD, GROUP, unknown), None, "{unknown:?}");
        }
    }

    #[test]
    fn listed_key_is_accepted_and_others_are_refused() {
        let tree = Tree::new("listed");
        let allowed = key(1);
        tree.authorize(&allowed.public_line("operator@boot-host"));
        let mut authorizer = FileAuthorizer::new(&tree.root);
        assert!(authorizer.authorized(OPERATOR, &allowed.public_key_blob()));
        // A key that is not listed, and the listed key for an account that does not exist.
        assert!(!authorizer.authorized(OPERATOR, &key(2).public_key_blob()));
        assert!(!authorizer.authorized("admin", &allowed.public_key_blob()));
        // root has no authorized_keys of its own: operator's file does not count for it.
        assert!(!authorizer.authorized("root", &allowed.public_key_blob()));
    }

    #[test]
    fn options_and_malformed_lines_grant_nothing() {
        let tree = Tree::new("options");
        let restricted = key(3);
        tree.authorize(&format!("from=\"10.0.0.1\" {}\nssh-ed25519 not-base64\n", restricted.public_line("")));
        assert!(!FileAuthorizer::new(&tree.root).authorized(OPERATOR, &restricted.public_key_blob()));
    }

    #[test]
    fn unsafe_ownership_or_modes_refuse_the_keys() {
        let allowed = key(4);
        let line = allowed.public_line("");
        let cases: [(&str, fn(&Tree)); 5] = [
            ("group-writable file", |t| fs::set_permissions(t.home().join(".ssh/authorized_keys"), fs::Permissions::from_mode(0o620)).unwrap()),
            ("world-writable .ssh", |t| fs::set_permissions(t.home().join(".ssh"), fs::Permissions::from_mode(0o703)).unwrap()),
            ("group-writable home", |t| fs::set_permissions(t.home(), fs::Permissions::from_mode(0o775)).unwrap()),
            ("world-writable /home", |t| fs::set_permissions(t.root.join("home"), fs::Permissions::from_mode(0o777)).unwrap()),
            ("symbolic link", |t| {
                let file = t.home().join(".ssh/authorized_keys");
                fs::rename(&file, t.root.join("elsewhere")).unwrap();
                std::os::unix::fs::symlink(t.root.join("elsewhere"), file).unwrap();
            }),
        ];
        for (name, spoil) in cases {
            let tree = Tree::new(&name.replace(' ', "-"));
            tree.authorize(&line);
            assert!(FileAuthorizer::new(&tree.root).authorized(OPERATOR, &allowed.public_key_blob()), "{name}: before");
            spoil(&tree);
            assert!(!FileAuthorizer::new(&tree.root).authorized(OPERATOR, &allowed.public_key_blob()), "{name}: after");
        }
        // Owned by another user: the account's checks name uid 0 or its own uid only.
        let tree = Tree::new("owner");
        tree.authorize(&line);
        let mut other = tree.account.clone();
        other.uid = tree.account.uid + 1;
        assert!(authorized_keys(&tree.root, &tree.account).is_ok());
        if tree.account.uid != 0 {
            assert!(authorized_keys(&tree.root, &other).is_err());
        }
    }

    #[test]
    fn root_never_logs_in() {
        let root = lookup(PASSWD, GROUP, "root").unwrap();
        assert!(may_log_in(&root).is_err());
        assert!(may_log_in(&lookup(PASSWD, GROUP, OPERATOR).unwrap()).is_ok());
        // Another name for uid 0 is root too.
        let toor = lookup("toor:x:0:0:toor:/root:/bin/sh\n", "", "toor").unwrap();
        assert!(may_log_in(&toor).is_err());
        // The authorizer refuses root before it reads any key file.
        let tree = Tree::new("root-refused");
        assert!(!FileAuthorizer::new(&tree.root).authorized("root", &key(6).public_key_blob()));
    }

    #[test]
    fn authorize_key_creates_the_home_once_with_private_modes() {
        let tree = Tree::new("authorize");
        fs::create_dir(tree.root.join("home")).unwrap();
        fs::set_permissions(tree.root.join("home"), fs::Permissions::from_mode(0o755)).unwrap();
        let operator_key = key(5);
        let line = operator_key.public_line("operator@boot-host");
        assert!(authorize_key(&tree.root, &tree.account, &operator_key.public_key_blob(), &line).unwrap());
        assert!(FileAuthorizer::new(&tree.root).authorized(OPERATOR, &operator_key.public_key_blob()));
        for (path, mode) in [(tree.home(), 0o700), (tree.home().join(".ssh"), 0o700), (tree.home().join(".ssh/authorized_keys"), 0o600)] {
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, mode, "{}", path.display());
        }
        // The same key again adds no second line.
        assert!(!authorize_key(&tree.root, &tree.account, &operator_key.public_key_blob(), &line).unwrap());
        assert_eq!(fs::read_to_string(tree.home().join(".ssh/authorized_keys")).unwrap().lines().count(), 1);
        // A home whose parent another user could write is not created.
        let tree = Tree::new("authorize-unsafe");
        fs::create_dir(tree.root.join("home")).unwrap();
        fs::set_permissions(tree.root.join("home"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(authorize_key(&tree.root, &tree.account, &operator_key.public_key_blob(), &line).is_err());
        assert!(!tree.home().exists());
    }

    /// Credentials that apply the POSIX rules: a privileged caller sets any
    /// id, an unprivileged one only its own; `fail` makes one call fail.
    #[derive(Default)]
    struct Fake {
        uid: u32,
        gid: u32,
        groups: Vec<u32>,
        calls: Vec<&'static str>,
        fail: Option<&'static str>,
        ignore_setuid: bool,
    }

    impl Credentials for Fake {
        fn set_groups(&mut self, groups: &[u32]) -> Result<(), i32> {
            self.calls.push("setgroups");
            if self.fail == Some("setgroups") || self.uid != 0 { return Err(1); }
            self.groups = groups.to_vec();
            Ok(())
        }
        fn set_gid(&mut self, gid: u32) -> Result<(), i32> {
            self.calls.push("setgid");
            if self.fail == Some("setgid") || self.uid != 0 && gid != self.gid { return Err(1); }
            self.gid = gid;
            Ok(())
        }
        fn set_uid(&mut self, uid: u32) -> Result<(), i32> {
            self.calls.push("setuid");
            if self.fail == Some("setuid") || self.uid != 0 && uid != self.uid { return Err(1); }
            if !self.ignore_setuid { self.uid = uid; }
            Ok(())
        }
        fn ids(&self) -> (u32, u32, u32, u32) {
            (self.uid, self.uid, self.gid, self.gid)
        }
        fn groups(&self) -> Vec<u32> {
            self.groups.clone()
        }
    }

    #[test]
    fn privilege_drop_sets_groups_then_gid_then_uid_and_cannot_regain_root() {
        let operator = lookup(PASSWD, GROUP, "operator").unwrap();
        let mut credentials = Fake::default();
        become_account(&operator, &mut credentials).unwrap();
        assert_eq!(credentials.calls, ["setgroups", "setgid", "setuid", "setuid"]);
        assert_eq!(credentials.ids(), (1000, 1000, 1000, 1000));
        assert_eq!(credentials.groups, vec![1000, 10]);
    }

    #[test]
    fn privilege_drop_fails_closed() {
        let operator = lookup(PASSWD, GROUP, "operator").unwrap();
        for step in ["setgroups", "setgid", "setuid"] {
            let mut credentials = Fake { fail: Some(step), ..Fake::default() };
            assert!(become_account(&operator, &mut credentials).is_err(), "{step}");
        }
        // A setuid that reports success without changing the ids is caught.
        let mut credentials = Fake { ignore_setuid: true, ..Fake::default() };
        assert!(become_account(&operator, &mut credentials).is_err());
        // Already unprivileged as another user: no id can be switched.
        let mut credentials = Fake { uid: 2000, gid: 2000, ..Fake::default() };
        assert!(become_account(&operator, &mut credentials).is_err());
    }

    #[test]
    fn login_environment_names_the_account() {
        let operator = lookup(PASSWD, GROUP, "operator").unwrap();
        let environment = login_environment(&operator, Some("xterm-256color"));
        assert!(environment.contains(&"HOME=/home/operator".to_string()));
        assert!(environment.contains(&"USER=operator".to_string()));
        assert!(environment.contains(&"TERM=xterm-256color".to_string()));
        assert!(!environment.iter().any(|entry| entry.starts_with("PATH=") && entry.contains("/sbin")));
        assert!(!login_environment(&operator, Some("bad term")).iter().any(|e| e.starts_with("TERM=")));
    }
}
