//! The machine state store, `/.breenix-shared`, on a persistent install.
//!
//! An upgrade copies the new image's `/etc` into a new generation, so state the
//! machine itself owns lives in a store on the system partition that each
//! generation binds at `/.breenix-shared` (kernel/src/fs/ext2/generation.rs):
//!
//! - `etc/passwd`, `etc/group`, `etc/shadow`: the machine's own account lines.
//!   Each replaces the generation's line for the same name, or is added.
//! - `etc/ssh/`: the SSH host key files, copied into `/etc/ssh`.
//! - `var/lib/accounts/pending/<account>`: a public key line `install-persistent`
//!   staged for the account. It is added to the account's
//!   `~/.ssh/authorized_keys` once the home partition is mounted, then removed.
//!
//! init calls `reconcile` at boot, before bsshd starts; bsshd calls
//! `keep_host_key` after it generates a host key, so the next generation gets
//! the same key. The store and each file read from it must be root's,
//! writable by root only and not a symbolic link; the store and the private
//! files in it must also be unreadable by others. Anything else is refused.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::account::{self, check_owned, open_checked, system_owners, Refusal};
use super::host_keys;
use super::keys;

/// The store, under the root.
pub const STORE: &str = ".breenix-shared";
/// The account files the store may hold, with the mode each has in `/etc`.
pub const ACCOUNT_FILES: [(&str, u32); 3] = [("passwd", 0o644), ("group", 0o644), ("shadow", 0o600)];
/// Staged public keys, one file per account, under the store.
pub const PENDING: &str = "var/lib/accounts/pending";
/// The largest store file read.
const MAX_FILE: u64 = 256 * 1024;

/// What `reconcile` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Accounts from the store's passwd that `/etc/passwd` now holds.
    pub accounts: Vec<String>,
    /// Host key files copied into `/etc/ssh`.
    pub host_keys: Vec<String>,
    /// Staged keys added to authorized_keys: the account and the key's fingerprint.
    pub authorized: Vec<(String, String)>,
    /// Entries refused or left for a later boot, one sentence each.
    pub problems: Vec<String>,
}

fn refuse<T>(text: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(text.into()))
}

/// The store directory, if there is one: root's, and private to root.
fn store(root: &Path) -> Result<Option<PathBuf>, Refusal> {
    let path = root.join(STORE);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return refuse(format!("{}: {}", path.display(), e)),
        Ok(_) => {}
    }
    let meta = check_owned(&path, system_owners(), true)?;
    if meta.mode() & 0o077 != 0 {
        return refuse(format!("{} must be accessible by root only (mode {:o})", path.display(), meta.mode() & 0o7777));
    }
    Ok(Some(path))
}

/// The text of a store file, or no text when it is absent. It must be root's and
/// writable by root only; with `private`, readable by root only too.
fn read_store_file(path: &Path, private: bool) -> Result<Option<(String, u32)>, Refusal> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return refuse(format!("{}: {}", path.display(), e)),
        Ok(_) => {}
    }
    let mut file = open_checked(path, system_owners(), false)?;
    let mode = file.metadata().map(|meta| meta.mode() & 0o7777).or_else(|e| refuse(format!("{}: {}", path.display(), e)))?;
    if private && mode & 0o077 != 0 {
        return refuse(format!("{} must be readable by root only (mode {:o})", path.display(), mode));
    }
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(MAX_FILE + 1)
        .read_to_string(&mut text)
        .or_else(|e| refuse(format!("{}: {}", path.display(), e)))?;
    if text.len() as u64 > MAX_FILE {
        return refuse(format!("{} is larger than {} bytes", path.display(), MAX_FILE));
    }
    Ok(Some((text, mode)))
}

fn sync_directory(path: &Path) -> Result<(), Refusal> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .or_else(|e| refuse(format!("sync {}: {}", path.display(), e)))
}

/// Replace `path` with `text` at `mode`: written beside it, flushed, renamed over it.
fn replace_file(path: &Path, text: &str, mode: u32) -> Result<(), Refusal> {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".new");
    let staging = PathBuf::from(staging);
    match fs::remove_file(&staging) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return refuse(format!("remove {}: {}", staging.display(), e)),
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&staging)
        .or_else(|e| refuse(format!("create {}: {}", staging.display(), e)))?;
    // The creation mode is masked by the umask; set it exactly.
    file.set_permissions(fs::Permissions::from_mode(mode))
        .and_then(|_| file.write_all(text.as_bytes()))
        .and_then(|_| file.sync_all())
        .or_else(|e| refuse(format!("write {}: {}", staging.display(), e)))?;
    fs::rename(&staging, path).or_else(|e| refuse(format!("rename {}: {}", path.display(), e)))?;
    sync_directory(path.parent().unwrap_or(Path::new("/")))
}

/// Create `path` as a directory at `mode` unless it exists; either way it must
/// be root's and writable by root only.
fn ensure_directory(path: &Path, mode: u32) -> Result<(), Refusal> {
    if let Err(e) = fs::symlink_metadata(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return refuse(format!("{}: {}", path.display(), e));
        }
        fs::create_dir(path).or_else(|e| refuse(format!("mkdir {}: {}", path.display(), e)))?;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .or_else(|e| refuse(format!("chmod {}: {}", path.display(), e)))?;
        sync_directory(path.parent().unwrap_or(Path::new("/")))?;
    }
    check_owned(path, system_owners(), true).map(|_| ())
}

/// The account a colon-separated database line names.
fn line_name(line: &str) -> &str {
    line.split(':').next().unwrap_or("")
}

/// `existing` with each of `entries` in place of the line naming the same
/// account, or appended when no line names it. Other lines are kept as they are.
pub fn merge_lines(existing: &str, entries: &[&str]) -> String {
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    for entry in entries {
        match lines.iter_mut().find(|line| line_name(line) == line_name(entry)) {
            Some(line) => *line = entry.to_string(),
            None => lines.push(entry.to_string()),
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The store's lines for one account file that may enter `/etc`: well formed,
/// for a valid name, and in passwd neither uid 0 nor a uid another account in
/// `existing` has. Refusals are added to `problems`.
fn accepted<'a>(file: &str, entries: &'a str, existing: &str, problems: &mut Vec<String>) -> Vec<&'a str> {
    let fields = match file {
        "passwd" => 7,
        "group" => 4,
        _ => 9,
    };
    let mut kept = Vec::new();
    for line in entries.lines().filter(|line| !line.trim().is_empty()) {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() != fields || !account::valid_name(parts[0]) {
            problems.push(format!("store {} line for '{}' is malformed", file, parts[0]));
            continue;
        }
        if file == "passwd" {
            let Ok(uid) = parts[2].parse::<u32>() else {
                problems.push(format!("store passwd line for '{}' has no numeric uid", parts[0]));
                continue;
            };
            let taken = existing.lines().chain(entries.lines()).any(|line| {
                let other: Vec<&str> = line.split(':').collect();
                other.len() == 7 && other[0] != parts[0] && other[2].parse::<u32>() == Ok(uid)
            });
            if uid == 0 || taken {
                problems.push(format!("store account '{}' would share uid {} with another account", parts[0], uid));
                continue;
            }
        }
        kept.push(line);
    }
    kept
}

/// Bring the store's state into `/etc` under `root`, and add staged keys when
/// `home_mounted` says the home partition is in place. Gives no report when
/// there is no store. A refused store is an error; refused entries in it are
/// reported in `Report::problems` and the rest is still applied.
pub fn reconcile(root: &Path, home_mounted: bool) -> Result<Option<Report>, Refusal> {
    let Some(store) = store(root)? else { return Ok(None) };
    let mut report = Report::default();

    // Read each account file before writing one, so a refused one leaves /etc as it was.
    let mut stored = Vec::new();
    for (file, mode) in ACCOUNT_FILES {
        if let Some((entries, _)) = read_store_file(&store.join("etc").join(file), mode == 0o600)? {
            stored.push((file, mode, entries));
        }
    }
    for (file, mode, entries) in stored {
        let target = root.join("etc").join(file);
        let existing = match fs::read_to_string(&target) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return refuse(format!("{}: {}", target.display(), e)),
        };
        let lines = accepted(file, &entries, &existing, &mut report.problems);
        let merged = merge_lines(&existing, &lines);
        if merged != existing {
            replace_file(&target, &merged, mode)?;
        }
        if file == "passwd" {
            report.accounts = lines.iter().map(|line| line_name(line).to_string()).collect();
        }
    }

    let keys = store.join("etc/ssh");
    if fs::symlink_metadata(&keys).is_ok() {
        check_owned(&keys, system_owners(), true)?;
        let mut names: Vec<String> = fs::read_dir(&keys)
            .or_else(|e| refuse(format!("{}: {}", keys.display(), e)))?
            .filter_map(|entry| entry.ok().and_then(|entry| entry.file_name().into_string().ok()))
            .collect();
        names.sort();
        let directory = root.join(host_keys::DIRECTORY);
        ensure_directory(&directory, 0o755)?;
        for name in names {
            let private = !name.ends_with(".pub");
            let Some((text, mode)) = read_store_file(&keys.join(&name), private)? else { continue };
            let target = directory.join(&name);
            if fs::read_to_string(&target).ok().as_deref() != Some(text.as_str()) {
                replace_file(&target, &text, mode & 0o777)?;
            }
            report.host_keys.push(name);
        }
    }

    let pending = store.join(PENDING);
    if fs::symlink_metadata(&pending).is_ok() {
        check_owned(&pending, system_owners(), true)?;
        let mut names: Vec<String> = fs::read_dir(&pending)
            .or_else(|e| refuse(format!("{}: {}", pending.display(), e)))?
            .filter_map(|entry| entry.ok().and_then(|entry| entry.file_name().into_string().ok()))
            .collect();
        names.sort();
        for name in names {
            if !account::valid_name(&name) {
                report.problems.push(format!("staged key file '{}' does not name an account", name));
                continue;
            }
            if !home_mounted {
                report.problems.push(format!("the home partition is not mounted; the staged key for '{}' waits", name));
                continue;
            }
            let path = pending.join(&name);
            let result = read_store_file(&path, true).and_then(|text| {
                let text = text.map(|(text, _)| text).unwrap_or_default();
                let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
                let [line] = lines.as_slice() else {
                    return refuse(format!("{} must hold one public key line", path.display()));
                };
                let blob = keys::parse_public_line(line)
                    .ok_or_else(|| Refusal(format!("{} is not an ssh-ed25519 or ssh-rsa public key", path.display())))?;
                let account = account::read_account(root, &name)?;
                account::may_log_in(&account)?;
                account::authorize_key(root, &account, &blob, line)?;
                Ok(keys::fingerprint_text(&blob))
            });
            match result {
                Ok(fingerprint) => {
                    fs::remove_file(&path).or_else(|e| refuse(format!("remove {}: {}", path.display(), e)))?;
                    sync_directory(&pending)?;
                    report.authorized.push((name, fingerprint));
                }
                Err(why) => report.problems.push(format!("staged key for '{}' not added: {}", name, why)),
            }
        }
    }
    Ok(Some(report))
}

/// Copy the host key files from `/etc/ssh` into the store, when there is a
/// store and it lacks them, so later generations keep the key. Returns whether
/// anything was written.
pub fn keep_host_key(root: &Path) -> Result<bool, Refusal> {
    let Some(store) = store(root)? else { return Ok(false) };
    ensure_directory(&store.join("etc"), 0o755)?;
    let keys = store.join("etc/ssh");
    ensure_directory(&keys, 0o700)?;
    let mut wrote = false;
    for name in [host_keys::PRIVATE.to_string(), format!("{}.pub", host_keys::PRIVATE)] {
        let target = keys.join(&name);
        if fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        let source = root.join(host_keys::DIRECTORY).join(&name);
        let Some((text, mode)) = read_store_file(&source, !name.ends_with(".pub"))? else {
            return refuse(format!("{} is missing", source.display()));
        };
        replace_file(&target, &text, mode & 0o777)?;
        wrote = true;
    }
    Ok(wrote)
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" {
        fn getuid() -> u32;
        fn getgid() -> u32;
    }

    /// A scratch root with a generation's /etc and a store, owned by the test
    /// user, who stands in for root (files cannot be given away without privilege).
    struct Tree {
        root: PathBuf,
        uid: u32,
        gid: u32,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("bssh-store-{}-{}", name, std::process::id()));
            let _ = fs::remove_dir_all(&root);
            for dir in ["etc", "home", ".breenix-shared/etc", ".breenix-shared/var/lib/accounts/pending"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            for dir in ["", "etc", "home", ".breenix-shared/etc", ".breenix-shared/var", ".breenix-shared/var/lib",
                        ".breenix-shared/var/lib/accounts", ".breenix-shared/var/lib/accounts/pending"] {
                fs::set_permissions(root.join(dir), fs::Permissions::from_mode(0o755)).unwrap();
            }
            fs::set_permissions(root.join(STORE), fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(root.join("etc/passwd"), "root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534:nobody:/nonexistent:/bin/false\n").unwrap();
            fs::write(root.join("etc/group"), "root:x:0:\nnobody:x:65534:\n").unwrap();
            fs::write(root.join("etc/shadow"), "root:$6$console:::::::\n").unwrap();
            let (uid, gid) = unsafe { (getuid(), getgid()) };
            Tree { root, uid, gid }
        }

        fn store_file(&self, path: &str, text: &str, mode: u32) {
            let path = self.root.join(STORE).join(path);
            fs::write(&path, text).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        }

        /// The operator account's lines and staged key, as install-persistent writes them.
        fn stage_operator(&self, key: &keys::Ed25519Key) {
            self.store_file("etc/passwd", &format!("operator:x:{}:{}:operator:/home/operator:/bin/sh\n", self.uid, self.gid), 0o644);
            self.store_file("etc/group", &format!("operator:x:{}:\n", self.gid), 0o644);
            self.store_file("etc/shadow", "operator:!:::::::\n", 0o600);
            self.store_file(&format!("{}/operator", PENDING), &format!("{}\n", key.public_line("operator@boot-host")), 0o600);
        }

        fn read(&self, path: &str) -> String {
            fs::read_to_string(self.root.join(path)).unwrap()
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
    fn no_store_is_nothing_to_do() {
        let tree = Tree::new("absent");
        fs::remove_dir_all(tree.root.join(STORE)).unwrap();
        assert_eq!(reconcile(&tree.root, true).unwrap(), None);
        assert!(!keep_host_key(&tree.root).unwrap());
    }

    #[test]
    fn accounts_and_staged_key_enter_etc_and_home_once() {
        let tree = Tree::new("accounts");
        let operator = key(1);
        tree.stage_operator(&operator);
        let report = reconcile(&tree.root, true).unwrap().unwrap();
        assert_eq!(report.accounts, vec!["operator".to_string()]);
        assert_eq!(report.authorized, vec![("operator".to_string(), keys::fingerprint_text(&operator.public_key_blob()))]);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(tree.read("etc/passwd").starts_with("root:x:0:0:root:/root:/bin/sh\nnobody:"));
        assert!(tree.read("etc/passwd").ends_with(&format!("operator:x:{}:{}:operator:/home/operator:/bin/sh\n", tree.uid, tree.gid)));
        assert_eq!(tree.read("etc/shadow"), "root:$6$console:::::::\noperator:!:::::::\n");
        assert_eq!(fs::metadata(tree.root.join("etc/shadow")).unwrap().mode() & 0o777, 0o600);
        assert!(!tree.root.join(STORE).join(PENDING).join("operator").exists());
        let mut authorizer = account::FileAuthorizer::new(&tree.root);
        assert!(super::super::auth::Authorizer::authorized(&mut authorizer, "operator", &operator.public_key_blob()));
        // A second boot leaves the files as they were.
        let passwd = tree.read("etc/passwd");
        let report = reconcile(&tree.root, true).unwrap().unwrap();
        assert!(report.authorized.is_empty());
        assert_eq!(tree.read("etc/passwd"), passwd);
        assert_eq!(tree.read("home/operator/.ssh/authorized_keys").lines().count(), 1);
    }

    #[test]
    fn store_lines_replace_the_generations_line_for_the_same_account() {
        assert_eq!(merge_lines("root:x:0:\nwheel:x:10:\n", &["wheel:x:10:operator", "operator:x:1000:"]),
                   "root:x:0:\nwheel:x:10:operator\noperator:x:1000:\n");
        assert_eq!(merge_lines("", &["operator:x:1000:"]), "operator:x:1000:\n");
    }

    #[test]
    fn shell_upgrade_preserves_password_and_every_authorized_line() {
        let tree = Tree::new("shell-upgrade");
        let primary = key(11);
        tree.stage_operator(&primary);
        reconcile(&tree.root, true).unwrap();
        let path = tree.root.join("home/operator/.ssh/authorized_keys");
        let original = format!("# locally maintained keys\n{}\n{}", primary.public_line("primary"), key(12).public_line("second"));
        fs::write(&path, &original).unwrap();
        let shadow = "operator:$6$changed-locally:20000:0:99999:7:::\n";
        tree.store_file("etc/shadow", shadow, 0o600);
        tree.store_file("etc/passwd", &format!("operator:x:{}:{}:operator:/home/operator:/bin/bsh\n", tree.uid, tree.gid), 0o644);
        tree.store_file(&format!("{}/operator", PENDING), &primary.public_line("primary"), 0o600);
        let report = reconcile(&tree.root, true).unwrap().unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(tree.read("etc/passwd").ends_with("/bin/bsh\n"));
        assert!(tree.read("etc/shadow").ends_with(shadow));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn root_and_colliding_or_malformed_entries_are_refused() {
        let tree = Tree::new("collide");
        tree.store_file("etc/passwd", "toor:x:0:0:toor:/root:/bin/sh\nimposter:x:65534:1:x:/home/imposter:/bin/sh\nshort:x:5\n", 0o644);
        let report = reconcile(&tree.root, true).unwrap().unwrap();
        assert!(report.accounts.is_empty());
        assert_eq!(report.problems.len(), 3, "{:?}", report.problems);
        assert!(!tree.read("etc/passwd").contains("toor") && !tree.read("etc/passwd").contains("imposter"));
    }

    #[test]
    fn incoming_accounts_cannot_share_a_uid() {
        let tree = Tree::new("incoming-collision");
        tree.store_file("etc/passwd", "alice:x:1000:1000:alice:/home/alice:/bin/sh\nbob:x:1000:1001:bob:/home/bob:/bin/sh\n", 0o644);
        let report = reconcile(&tree.root, true).unwrap().unwrap();
        assert!(report.accounts.is_empty());
        assert_eq!(report.problems.len(), 2);
        let passwd = tree.read("etc/passwd");
        assert!(!passwd.contains("alice:") && !passwd.contains("bob:"));
    }

    #[test]
    fn unsafe_store_or_files_are_refused() {
        let tree = Tree::new("unsafe");
        tree.stage_operator(&key(2));
        fs::set_permissions(tree.root.join(STORE), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(reconcile(&tree.root, true).is_err());
        fs::set_permissions(tree.root.join(STORE), fs::Permissions::from_mode(0o700)).unwrap();
        // A shadow file others can read refuses the reconcile, before /etc is touched.
        tree.store_file("etc/shadow", "operator:!:::::::\n", 0o644);
        assert!(reconcile(&tree.root, true).is_err());
        tree.store_file("etc/shadow", "operator:!:::::::\n", 0o600);
        // A group-writable passwd likewise.
        tree.store_file("etc/passwd", &format!("operator:x:{}:{}:operator:/home/operator:/bin/sh\n", tree.uid, tree.gid), 0o664);
        assert!(reconcile(&tree.root, true).is_err());
        assert!(!tree.read("etc/passwd").contains("operator"));
    }

    #[test]
    fn staged_key_waits_for_the_home_partition_and_bad_keys_stay() {
        let tree = Tree::new("pending");
        tree.stage_operator(&key(3));
        let report = reconcile(&tree.root, false).unwrap().unwrap();
        assert!(report.authorized.is_empty());
        assert_eq!(report.problems.len(), 1);
        assert!(tree.root.join(STORE).join(PENDING).join("operator").exists());
        assert!(!tree.root.join("home/operator").exists());
        // A private key, or two lines, are not added and stay for the operator to see.
        for text in ["-----BEGIN OPENSSH PRIVATE KEY-----\n", "ssh-ed25519 AAAA\nssh-ed25519 BBBB\n"] {
            tree.store_file(&format!("{}/operator", PENDING), text, 0o600);
            let report = reconcile(&tree.root, true).unwrap().unwrap();
            assert!(report.authorized.is_empty() && report.problems.len() == 1, "{text:?}");
            assert!(tree.root.join(STORE).join(PENDING).join("operator").exists());
        }
        // A key staged for root is not added.
        tree.store_file(&format!("{}/root", PENDING), &key(4).public_line(""), 0o600);
        let report = reconcile(&tree.root, true).unwrap().unwrap();
        assert!(report.authorized.is_empty());
        assert!(!tree.root.join("root/.ssh").exists());
    }

    #[test]
    fn host_key_kept_in_the_store_is_the_next_generations_key() {
        let tree = Tree::new("hostkey");
        let (first, generated) = host_keys::load_or_generate(&tree.root, |buffer: &mut [u8]| {
            buffer.fill(9);
            Ok(())
        })
        .unwrap();
        assert!(generated);
        assert!(keep_host_key(&tree.root).unwrap());
        assert!(!keep_host_key(&tree.root).unwrap());
        let stored = tree.root.join(STORE).join("etc/ssh").join(host_keys::PRIVATE);
        assert_eq!(fs::metadata(&stored).unwrap().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(tree.root.join(STORE).join("etc/ssh")).unwrap().mode() & 0o777, 0o700);
        // An upgrade: the new generation's /etc has no key until init reconciles.
        fs::remove_dir_all(tree.root.join("etc/ssh")).unwrap();
        let report = reconcile(&tree.root, true).unwrap().unwrap();
        assert_eq!(report.host_keys, vec![host_keys::PRIVATE.to_string(), format!("{}.pub", host_keys::PRIVATE)]);
        let (second, generated) = host_keys::load_or_generate(&tree.root, |_: &mut [u8]| Err("must not be asked")).unwrap();
        assert!(!generated);
        assert_eq!(second.public_key_blob(), first.public_key_blob());
        // A stored private key others can read is not copied.
        fs::remove_dir_all(tree.root.join("etc/ssh")).unwrap();
        fs::set_permissions(&stored, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(reconcile(&tree.root, true).is_err());
    }
}
