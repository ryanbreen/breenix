//! Users & permissions: credentials, file access, sticky directories, set-ID exec,
//! signal permission, /proc and device permissions and the file creation mask, as POSIX
//! and `docs/users/design.md` (section 2) specify them.
//!
//! The suite runs as root, as PID 1, and the runner forks each case. A case that needs
//! another identity forks again: that child becomes a fixture account (`become_account`:
//! its supplementary groups, then setresgid and setresuid, then a check that setuid(0) is
//! refused), performs the operation and reports through a pipe. The fixture accounts are
//! IDs only: ua 2001 (in group ug 2100), ub 2002 and uw 2003 (in wheel, 10). Each case
//! makes its files in its own directory under /tmp, or under /home for the nosuid case,
//! and removes them when it ends. Error assertions read the raw system call return, so an
//! errno the library does not name is still reported.
use libbreenix::io;
use libbreenix::process::{self, ForkResult};
use libbreenix::suite::{case, category, check, fail, suite, CaseError, CaseResult, Suite};
use libbreenix::syscall::raw;
use libbreenix::types::Fd;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::mpsc;
use std::fs;
use std::path::Path;
use std::os::unix::fs::MetadataExt;

static SUITE: Suite = suite(
    "users",
    "Users & permissions",
    &[
        category("credentials", "Real, effective and saved IDs, supplementary groups and their thread-group commit", &[
            case("ids-after-drop", "A root process that becomes a user holds that user's real, effective and saved IDs and groups", ids_after_drop),
            case("no-regain-root", "A user cannot take user ID 0 back through setuid, setreuid or setresuid", no_regain_root),
            case("saved-toggle", "A process whose saved user ID differs from its real one switches its effective ID between them", saved_toggle),
            case("setres-rules", "Unprivileged setresuid and setresgid accept only the current three IDs, and a refusal changes nothing", setres_rules),
            case("setgroups-user-eperm", "setgroups by a user fails with EPERM and leaves the groups as they were", setgroups_user_eperm),
            case("threads-share-groups", "setgroups in one thread changes the supplementary groups of every thread of the process", threads_share_groups),
            case("orphan-thread-not-root", "A thread whose process cannot be found is refused, never checked as root (forcing point)", orphan_thread_not_root),
        ]),
        category("files", "File access classes, ownership, chmod, chown and set-ID clearing", &[
            case("other-0600-read-eacces", "Another user's 0600 file cannot be opened for reading (EACCES)", other_read_eacces),
            case("other-0600-write-eacces", "Another user's 0600 file cannot be opened for writing (EACCES)", other_write_eacces),
            case("group-0640-member", "A member of a 0640 file's group may read it and may not write it", group_member),
            case("group-0640-nonmember", "A user outside a 0640 file's group gets the other class: no read", group_nonmember),
            case("supplementary-access", "A supplementary group grants its group class", supplementary_access),
            case("root-exec-needs-x", "Root may not execute a file with no execute bit, and may with one in any class", root_exec_needs_x),
            case("search-denied", "A directory without search permission blocks the path through it and chdir into it", search_denied),
            case("create-owner", "A new file takes the creator's effective user and group IDs and the mode less the umask", create_owner),
            case("sgid-dir-group", "In a set-group-ID directory new files take its group and new directories its set-group-ID bit", sgid_dir_group),
            case("chmod-not-owner-eperm", "chmod of another user's file fails with EPERM", chmod_not_owner),
            case("chown-giveaway-eperm", "A user cannot give a file to another user (EPERM)", chown_giveaway),
            case("chgrp-nonmember-eperm", "The owner may change a file's group only to a group they belong to", chgrp_nonmember),
            case("write-clears-setid", "A user's write or truncate clears a file's set-user-ID and set-group-ID bits", write_clears_setid),
            case("mmap-shared-clears-setid", "A user's shared writable mapping clears a file's set-ID bits, and its pending stores confer no identity at exec", mmap_clears_setid),
            case("utimes-explicit-nonowner-eperm", "Explicit times need ownership (EPERM); setting both to now needs ownership or write (EACCES)", utimes_nonowner),
        ]),
        category("sticky", "Sticky world-writable directories: removal, rename, symlinks and hard links", &[
            case("tmp-1777", "/tmp is a root-owned sticky directory every user may write: mode 1777", tmp_1777),
            case("unlink-other-eperm", "In a sticky directory a user cannot remove another user's file (EPERM)", sticky_unlink),
            case("rename-other-eperm", "In a sticky directory a user cannot rename another user's file (EPERM)", sticky_rename),
            case("replace-other-eperm", "In a sticky directory a user cannot rename over another user's file (EPERM)", sticky_replace),
            case("dir-owner-may-remove", "The owner of a sticky directory may remove another user's file in it", sticky_dir_owner),
            case("symlink-follow-refused", "A symlink in a sticky directory is followed only by its owner or the directory's owner (EACCES)", sticky_symlink),
            case("hardlink-other-refused", "A user cannot hard-link a file they neither own nor may read and write (EPERM)", sticky_hardlink),
        ]),
        category("setid-exec", "Set-user-ID and set-group-ID execution, nosuid and AT_SECURE", &[
            case("setuid-euid", "A set-user-ID program runs with its owner as the effective user ID and the caller's real one", setuid_euid),
            case("setuid-saved", "A set-user-ID program switches between its real and saved user IDs", setuid_saved),
            case("setgid-egid", "A set-group-ID program runs with its group as the effective group ID", setgid_egid),
            case("setgid-no-gx-ignored", "A set-group-ID bit without group execute confers no group", setgid_no_gx),
            case("nosuid-home-ignored", "Set-ID bits confer nothing under /home, which is mounted nosuid,nodev", nosuid_home),
            case("shared-fs-no-gain", "An exec while another process shares the working directory gains no identity", shared_fs_no_gain),
            case("at-secure", "A set-ID program starts with AT_SECURE set, and an ordinary one without it", at_secure),
            case("closed-stdio-opened", "A set-ID program started with descriptors 0 to 2 closed finds them open on /dev/null", closed_stdio),
        ]),
        category("signals", "Signal and priority permission between users", &[
            case("kill-other-eperm", "A user cannot signal another user's process (EPERM), which keeps running", kill_other),
            case("kill-probe-eperm", "kill(pid, 0) on another user's process fails with EPERM", kill_probe),
            case("kill-setuid-child", "A user may signal a set-user-ID program they started", kill_setuid_child),
            case("root-kills-any", "Root may signal any user's process", root_kills_any),
            case("renice-other-eperm", "A user cannot change another user's priority (EPERM)", renice_other),
        ]),
        category("proc-dev", "/proc, device nodes and privileged ports by owner and mode", &[
            case("proc-exe-other-eacces", "A user cannot read another user's /proc/<pid>/exe (EACCES) and may read their own", proc_exe_other),
            case("kernel-output-other-eacces", "A user cannot read another user's thread kernel_output (EACCES) and may read their own", kernel_output_other),
            case("kmsg-user-eacces", "/proc/kmsg is root's alone (EACCES)", kmsg_user),
            case("trace-user-eacces", "/proc/trace is root's alone (EACCES)", trace_user),
            case("vcs-user-eacces", "The console and the screen devices are root's 0600 nodes a user cannot open (EACCES)", vcs_user),
            case("tcp-low-port-eacces", "A user cannot bind a TCP port below 1024 (EACCES); root can", tcp_low_port),
            case("home-nodev-device-eacces", "A device node under /home, mounted nodev, opens no device (EACCES), and only root may make one", home_nodev_device),
            case("window-other-eperm", "A user cannot list, read, map or exchange input with another user's window (EPERM)", window_other),
        ]),
        category("umask", "File creation mask across files, directories, fork and threads", &[
            case("umask-file", "A new file's mode is the requested mode less the umask", umask_file),
            case("umask-dir", "A new directory's mode is the requested mode less the umask", umask_dir),
            case("umask-fork", "A forked child inherits the umask", umask_fork),
            case("umask-threads", "umask in one thread sets the mask of every thread of the process", umask_threads),
        ]),
    category("layout", "The standard filesystem layout with its owners and modes", &[
        case("fhs-paths", "Standard layout paths have their specified owners and modes", fhs_paths),
        case("shadow-0600", "The shadow database is a root-owned regular file at mode 0600", shadow),
        case("sudo-policy-0440", "The admin policy is a root-owned regular file at mode 0440", sudo_policy),
        case("setuid-set", "Image set-ID files are limited to the design table", setuid_set),
        case("run-cleared", "Startup removes stale runtime and temporary state", run_cleared),
    ]),
    category("accounts", "passwd, group and shadow files and the account tools", &[
        case("useradd-entries", "useradd creates matching passwd, group and locked shadow entries", accounts_entries),
        case("useradd-home-0700", "useradd -m copies the skeleton into a user-owned 0700 home", accounts_home),
        case("useradd-duplicate-refused", "Duplicate names and IDs are refused without changing any database", accounts_duplicate),
        case("useradd-options", "Explicit IDs, groups, shell and home are stored and non-root useradd is refused", accounts_options),
        case("userdel-removes", "userdel removes account entries, memberships and an owned home", accounts_delete),
        case("userdel-busy-refused", "userdel refuses a user with running processes", accounts_busy),
        case("userdel-home-owner-refused", "userdel -r refuses a home owned by another identity", accounts_home_owner),
        case("usermod-groups", "usermod replaces or appends supplementary groups and preserves other members", accounts_groups),
        case("usermod-lock-expire", "usermod locks and unlocks a password, changes shell and sets account expiration", accounts_modify),
        case("groupadd-groupdel", "Group tools allocate IDs, refuse duplicates and retain primary groups", accounts_group_tools),
        case("lock-serializes", "Concurrent useradds wait for the password lock and retain both new accounts", accounts_serializes),
        case("libc-lock-shared", "libc lckpwdf and the library contend on the same POSIX record lock", accounts_libc_lock),
        case("interrupted-write-intact", "Exit at every write boundary leaves complete old or new files and publishes shadow before passwd", accounts_interrupted),
        case("passwd-own", "passwd library verifies the old password and writes a fresh 100000-round hash for the caller", accounts_password),
        case("passwd-other-refused", "passwd library refuses another account, a wrong old password and a short replacement", accounts_password_refused),
        case("id-output", "id and groups resolve a newly created account through libc", accounts_id),
        case("malformed-isolated", "A malformed account is disabled while a valid neighbour remains usable", accounts_malformed),
        case("audit-log", "Account tools append a successful change to the protected authentication log", accounts_audit),
    ]),
    category("su", "su: passwords, wheel, login environment and the auth log", &[
        case("setuid-set", "Exactly su, passwd and sudo are installed set-user-ID root", admin_setuid_set),
        case("su-wrong-password-refused", "su refuses a wrong target password and accepts the right one", su_wrong_password),
        case("su-root-ok", "A wheel member becomes root with root's password", su_root_ok),
        case("su-nonwheel-refused", "su refuses root to a caller outside wheel without asking for a password", su_nonwheel_refused),
        case("su-dash-env", "su - gives the target's clean login environment and home; plain su keeps the caller's", su_dash_env),
        case("su-logged", "su successes and refusals are recorded in the protected auth log without passwords", su_logged),
        case("passwd-command", "The set-user-ID passwd command changes the caller's own password and no other account's", passwd_command),
    ]),
    category("sudo", "sudo-style administration: policy, password cache, environment and the auth log", &[
        case("sudo-wheel-ok", "A wheel member runs a command as root with their own password", sudo_wheel_ok),
        case("sudo-nonwheel-refused", "sudo refuses a caller no rule permits without asking for a password", sudo_nonwheel_refused),
        case("sudo-wrong-password", "sudo refuses a wrong password", sudo_wrong_password),
        case("sudo-env-reset", "The command gets a reset environment, the secure PATH and umask 022", sudo_env_reset),
        case("sudo-bad-policy-mode-refused", "sudo refuses to run under a policy with the wrong mode or one that does not parse", sudo_bad_policy_mode),
        case("sudo-cache-session", "An authentication is remembered for the session and terminal only", sudo_cache_session),
        case("sudo-policy-rules", "A rule's target and command decide", sudo_policy_rules),
        case("sudo-first-match", "Of two rules that both permit the call, the first decides, in each order", sudo_first_match),
        case("sudo-argv-no-shell", "The permitted command runs with its arguments as given, never through a shell, with and without -i", sudo_argv_no_shell),
        case("sudo-logged", "sudo successes and refusals are recorded with the command and without passwords", sudo_logged),
    ]),
    category("login", "Console login as any account, tty ownership and umask", &[
        case("login-user", "A fixture account logs in through getty and login with its IDs, groups, environment, home and login shell", login_user),
        case("login-bad-password-refused", "A wrong password is refused with Login incorrect and the right one then logs in", login_bad_password),
        case("login-root-refused", "On a pseudo-terminal line login refuses root under the shipped ROOT_LOGIN no, and the same password logs in there under ROOT_LOGIN yes", login_root_refused),
        case("login-locked-refused", "A locked password is refused at login and the unlocked one logs in", login_locked_refused),
        case("login-expired-refused", "An expired account is refused at login, and logs in once its expiry is cleared", login_expired_refused),
        case("login-tty-owned", "The session's terminal belongs to the account, group tty, mode 0620", login_tty_owned),
        case("login-umask", "A session starts with the umask from /etc/login.defs", login_umask),
        case("logout-tty-restored", "After logout getty returns the terminal to root at 0620 and records the logout", logout_tty_restored),
        case("logout-revokes-terminal", "A descriptor the old session kept on the line reads end of file, cannot write and cannot reopen it once getty serves the next login", logout_revokes_terminal),
    ]),
    category("ssh", "SSH sessions as the account: identity, groups, PTY owner and key-file modes", &[
        case("ssh-identity", "An SSH command runs as the account with its IDs, groups, environment and home", ssh_identity),
        case("ssh-root-refused", "bsshd refuses root's key, and the same key logs in as an ordinary account", ssh_root_refused),
        case("ssh-groups", "An SSH session holds the account's supplementary groups", ssh_groups),
        case("ssh-pty-owner", "An interactive SSH session's PTY belongs to the account, group tty, mode 0620", ssh_pty_owner),
        case("ssh-bad-key-mode-refused", "A group-writable authorized_keys grants no login until its mode is 0600", ssh_bad_key_mode_refused),
    ]),
    ],
);

fn attrs(path: &Path, mode: u32, uid: u32, gid: u32, directory: bool) -> CaseResult {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    check(if directory { meta.is_dir() } else { meta.is_file() }, &format!("{}: wrong kind", path.display()))?;
    check(meta.uid() == uid && meta.gid() == gid && meta.mode() & 0o7777 == mode,
        &format!("{}: owner {}:{} mode {:o}, expected {}:{} {:o}", path.display(), meta.uid(), meta.gid(), meta.mode() & 0o7777, uid, gid, mode))
}
fn fhs_paths() -> CaseResult {
    for line in libbreenix::layout::TABLE.lines() {
        let row: Vec<_> = line.split_whitespace().collect();
        if row.len() != 5 || !matches!(row[0], "dir" | "file") { continue; }
        let mode = u32::from_str_radix(row[2], 8).map_err(|e| e.to_string())?;
        attrs(&Path::new("/").join(row[1]), mode, row[3].parse().map_err(|_| "bad uid")?, row[4].parse().map_err(|_| "bad gid")?, row[0] == "dir")?;
    }
    for line in fs::read_to_string("/etc/passwd").map_err(|e| e.to_string())?.lines() {
        let fields: Vec<_> = line.split(':').collect();
        if fields.len() == 7 && fields[5].starts_with("/home/") {
            attrs(Path::new(fields[5]), 0o700, fields[2].parse().map_err(|_| "bad account uid")?,
                fields[3].parse().map_err(|_| "bad account gid")?, true)?;
        }
    }
    check(!Path::new("/test").exists() && !Path::new("/deep").exists() && !Path::new("/usr/local/test").exists()
        && !Path::new("/usr/local/libc").exists(), "test fixtures remain in administrator paths")
}
fn shadow() -> CaseResult { attrs(Path::new("/etc/shadow"), 0o600, 0, 0, false) }
fn sudo_policy() -> CaseResult { attrs(Path::new("/etc/sudo.policy"), 0o440, 0, 0, false) }
fn setuid_set() -> CaseResult {
    libbreenix::layout::check_setid(Path::new("/"), true)?;
    Ok(())
}
fn admin_setuid_set() -> CaseResult {
    setuid_set()?;
    for path in ["/bin/su", "/usr/bin/passwd", "/usr/bin/sudo"] {
        attrs(Path::new(path), 0o4755, 0, 0, false)?;
    }
    Ok(())
}
fn run_cleared() -> CaseResult {
    check(!Path::new("/run/layout-stale").exists() && !Path::new("/tmp/layout-stale").exists(), "startup left stale state")
}

fn main() { SUITE.run() }

// ---------------------------------------------------------------------------
// Fixture accounts and system calls.

const UA: u32 = 2001;
const UB: u32 = 2002;
const UW: u32 = 2003;
const UG: u32 = 2100;
const WHEEL: u32 = 10;
const KEEP: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Account { uid: u32, gid: u32, groups: &'static [u32] }
const A: Account = Account { uid: UA, gid: UA, groups: &[UG] };
const B: Account = Account { uid: UB, gid: UB, groups: &[] };
const W: Account = Account { uid: UW, gid: UW, groups: &[WHEEL] };

/// The exec target the set-ID cases copy (`users_exec.rs`).
const HELPER: &str = "/usr/libexec/breenix/users-exec_test";

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const WRITE: u64 = 1;
    pub const CLOSE: u64 = 3;
    pub const MMAP: u64 = 9;
    pub const MPROTECT: u64 = 10;
    pub const FSYNC: u64 = 74;
    pub const GETPID: u64 = 39;
    pub const SOCKET: u64 = 41;
    pub const BIND: u64 = 49;
    pub const CLONE: u64 = 56;
    pub const EXECVE: u64 = 59;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const FTRUNCATE: u64 = 77;
    pub const CHDIR: u64 = 80;
    pub const UMASK: u64 = 95;
    pub const SETUID: u64 = 105;
    pub const SETREUID: u64 = 113;
    pub const GETGROUPS: u64 = 115;
    pub const SETGROUPS: u64 = 116;
    pub const SETRESUID: u64 = 117;
    pub const GETRESUID: u64 = 118;
    pub const SETRESGID: u64 = 119;
    pub const GETRESGID: u64 = 120;
    pub const STATFS: u64 = 137;
    pub const FSTATFS: u64 = 138;
    pub const SETPRIORITY: u64 = 141;
    pub const GETTID: u64 = 186;
    pub const OPENAT: u64 = 257;
    pub const MKDIRAT: u64 = 258;
    pub const MKNODAT: u64 = 259;
    pub const FCHOWNAT: u64 = 260;
    pub const NEWFSTATAT: u64 = 262;
    pub const UNLINKAT: u64 = 263;
    pub const RENAMEAT: u64 = 264;
    pub const LINKAT: u64 = 265;
    pub const SYMLINKAT: u64 = 266;
    pub const READLINKAT: u64 = 267;
    pub const FCHMODAT: u64 = 268;
    pub const UTIMENSAT: u64 = 280;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const WRITE: u64 = 64;
    pub const CLOSE: u64 = 57;
    pub const MMAP: u64 = 222;
    pub const MPROTECT: u64 = 226;
    pub const FSYNC: u64 = 82;
    pub const GETPID: u64 = 172;
    pub const SOCKET: u64 = 198;
    pub const BIND: u64 = 200;
    pub const CLONE: u64 = 220;
    pub const EXECVE: u64 = 221;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const FTRUNCATE: u64 = 46;
    pub const CHDIR: u64 = 49;
    pub const UMASK: u64 = 166;
    pub const SETUID: u64 = 146;
    pub const SETREUID: u64 = 145;
    pub const GETGROUPS: u64 = 158;
    pub const SETGROUPS: u64 = 159;
    pub const SETRESUID: u64 = 147;
    pub const GETRESUID: u64 = 148;
    pub const SETRESGID: u64 = 149;
    pub const GETRESGID: u64 = 150;
    pub const STATFS: u64 = 43;
    pub const FSTATFS: u64 = 44;
    pub const SETPRIORITY: u64 = 140;
    pub const GETTID: u64 = 178;
    pub const OPENAT: u64 = 56;
    pub const MKDIRAT: u64 = 34;
    pub const MKNODAT: u64 = 33;
    pub const FCHOWNAT: u64 = 54;
    pub const NEWFSTATAT: u64 = 79;
    pub const UNLINKAT: u64 = 35;
    pub const RENAMEAT: u64 = 38;
    pub const LINKAT: u64 = 37;
    pub const SYMLINKAT: u64 = 36;
    pub const READLINKAT: u64 = 78;
    pub const FCHMODAT: u64 = 53;
    pub const UTIMENSAT: u64 = 88;
}
/// Suite-mode test controls, the same number on both architectures.
const TEST_CONTROL: u64 = 415;
const TC_ARM: u64 = 1;
const TC_FIRED: u64 = 3;
/// The kernel's `cred-orphan` point: the armed thread's next credential lookup finds no process.
const CRED_ORPHAN: u32 = 32;

const EPERM: i64 = 1;
const ESRCH: i64 = 3;
const EACCES: i64 = 13;
#[cfg(target_arch = "x86_64")]
const EINVAL: i64 = 22;

const AT_FDCWD: u64 = (-100i64) as u64;
const AT_REMOVEDIR: u64 = 0x200;
const O_RDONLY: u64 = 0;
const O_WRONLY: u64 = 1;
const O_RDWR: u64 = 2;
const O_CREAT: u64 = 0o100;
const O_EXCL: u64 = 0o200;
const O_CLOEXEC: u64 = 0x80000;
const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const MAP_SHARED: u64 = 1;
const S_IFCHR: u32 = 0o020000;
/// /dev/null's device number, major 1 minor 3.
const NULL_RDEV: u64 = 0x103;
/// The framebuffer and window system call, the same number on both architectures.
const FBDRAW: u64 = 411;
const CLONE_FS: u64 = 0x200;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;
const SIGCHLD: u64 = 17;
const WNOHANG: i32 = 1;
const PRIO_PROCESS: u64 = 0;
const UTIME_NOW: i64 = (1 << 30) - 1;
const ST_NOSUID: u64 = 2;
const ST_NODEV: u64 = 4;

type Checked = Result<(), String>;

fn sc(n: u64, args: &[u64]) -> i64 {
    let a = |i: usize| args.get(i).copied().unwrap_or(0);
    // SAFETY: the callers keep the buffers their arguments point to alive through the call.
    unsafe { raw::syscall6(n, a(0), a(1), a(2), a(3), a(4), a(5)) as i64 }
}

fn errname(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 5 => "EIO", 9 => "EBADF", 13 => "EACCES", 17 => "EEXIST",
        20 => "ENOTDIR", 22 => "EINVAL", 38 => "ENOSYS", 98 => "EADDRINUSE",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A raw return as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if ret < 0 { errname(-ret) } else { ret.to_string() } }

/// Succeed when `ret` is not an error; otherwise name the call.
fn ok(what: &str, ret: i64) -> Result<i64, String> {
    if ret < 0 { Err(format!("{what} failed with {}", errname(-ret))) } else { Ok(ret) }
}

/// Succeed when `ret` is the error `errno`.
fn refused(what: &str, ret: i64, errno: i64) -> Checked {
    if ret == -errno { Ok(()) } else { Err(format!("{what}: expected {}, got {}", errname(errno), shown(ret))) }
}

fn want(cond: bool, msg: impl Into<String>) -> Checked { if cond { Ok(()) } else { Err(msg.into()) } }

fn cpath(path: &str) -> CString { CString::new(path).expect("path without NUL") }

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }
fn gettid() -> i64 { sc(nr::GETTID, &[]) }
fn kill(pid: i32, sig: i32) -> i64 { sc(nr::KILL, &[pid as i64 as u64, sig as u64]) }
fn umask(mask: u32) -> u32 { sc(nr::UMASK, &[mask as u64]) as u32 }
fn open(path: &str, flags: u64, mode: u32) -> i64 { let c = cpath(path); sc(nr::OPENAT, &[AT_FDCWD, c.as_ptr() as u64, flags, mode as u64]) }
fn close(fd: i64) { if fd >= 0 { sc(nr::CLOSE, &[fd as u64]); } }
/// Open `path` with `flags` and close it again: the open's raw return.
fn try_open(path: &str, flags: u64) -> i64 { let fd = open(path, flags, 0); close(fd); fd.min(0) }
fn mkdir(path: &str, mode: u32) -> i64 { let c = cpath(path); sc(nr::MKDIRAT, &[AT_FDCWD, c.as_ptr() as u64, mode as u64]) }
fn unlink(path: &str) -> i64 { let c = cpath(path); sc(nr::UNLINKAT, &[AT_FDCWD, c.as_ptr() as u64, 0]) }
fn rmdir(path: &str) -> i64 { let c = cpath(path); sc(nr::UNLINKAT, &[AT_FDCWD, c.as_ptr() as u64, AT_REMOVEDIR]) }
fn rename(from: &str, to: &str) -> i64 {
    let (f, t) = (cpath(from), cpath(to));
    sc(nr::RENAMEAT, &[AT_FDCWD, f.as_ptr() as u64, AT_FDCWD, t.as_ptr() as u64])
}
fn link(from: &str, to: &str) -> i64 {
    let (f, t) = (cpath(from), cpath(to));
    sc(nr::LINKAT, &[AT_FDCWD, f.as_ptr() as u64, AT_FDCWD, t.as_ptr() as u64, 0])
}
fn symlink(target: &str, at: &str) -> i64 {
    let (t, a) = (cpath(target), cpath(at));
    sc(nr::SYMLINKAT, &[t.as_ptr() as u64, AT_FDCWD, a.as_ptr() as u64])
}
fn readlink(path: &str) -> Result<String, i64> {
    let c = cpath(path);
    let mut buf = [0u8; 512];
    let r = sc(nr::READLINKAT, &[AT_FDCWD, c.as_ptr() as u64, buf.as_mut_ptr() as u64, buf.len() as u64]);
    if r < 0 { Err(r) } else { Ok(String::from_utf8_lossy(&buf[..r as usize]).into_owned()) }
}
fn chmod(path: &str, mode: u32) -> i64 { let c = cpath(path); sc(nr::FCHMODAT, &[AT_FDCWD, c.as_ptr() as u64, mode as u64, 0]) }
fn chown(path: &str, uid: u32, gid: u32) -> i64 {
    let c = cpath(path);
    sc(nr::FCHOWNAT, &[AT_FDCWD, c.as_ptr() as u64, uid as u64, gid as u64, 0])
}
fn chdir(path: &str) -> i64 { let c = cpath(path); sc(nr::CHDIR, &[c.as_ptr() as u64]) }
fn stat(path: &str) -> Result<libbreenix::fs::Stat, String> {
    let c = cpath(path);
    let mut st = libbreenix::fs::Stat::default();
    let r = sc(nr::NEWFSTATAT, &[AT_FDCWD, c.as_ptr() as u64, &mut st as *mut _ as u64, 0]);
    if r < 0 { Err(format!("stat {path} failed with {}", errname(-r))) } else { Ok(st) }
}
fn mode_of(path: &str) -> Result<u32, String> { stat(path).map(|st| st.st_mode) }
fn write_fd(fd: i64, bytes: &[u8]) -> i64 { sc(nr::WRITE, &[fd as u64, bytes.as_ptr() as u64, bytes.len() as u64]) }
fn setuid(uid: u32) -> i64 { sc(nr::SETUID, &[uid as u64]) }
fn setreuid(real: u32, effective: u32) -> i64 { sc(nr::SETREUID, &[real as u64, effective as u64]) }
fn setresuid(r: u32, e: u32, s: u32) -> i64 { sc(nr::SETRESUID, &[r as u64, e as u64, s as u64]) }
fn setresgid(r: u32, e: u32, s: u32) -> i64 { sc(nr::SETRESGID, &[r as u64, e as u64, s as u64]) }
fn setgroups(list: &[u32]) -> i64 { sc(nr::SETGROUPS, &[list.len() as u64, list.as_ptr() as u64]) }
fn getgroups() -> Result<Vec<u32>, String> {
    let mut list = [0u32; 64];
    let r = sc(nr::GETGROUPS, &[list.len() as u64, list.as_mut_ptr() as u64]);
    if r < 0 { return Err(format!("getgroups failed with {}", errname(-r))); }
    let mut groups = list[..r as usize].to_vec();
    groups.sort_unstable();
    Ok(groups)
}
fn res(n: u64) -> [u32; 3] {
    let mut v = [KEEP; 3];
    sc(n, &[&mut v[0] as *mut u32 as u64, &mut v[1] as *mut u32 as u64, &mut v[2] as *mut u32 as u64]);
    v
}
fn resuid() -> [u32; 3] { res(nr::GETRESUID) }
fn resgid() -> [u32; 3] { res(nr::GETRESGID) }
fn utimens(path: &str, times: Option<[i64; 4]>) -> i64 {
    let c = cpath(path);
    let ptr = times.as_ref().map_or(0, |t| t.as_ptr() as u64);
    sc(nr::UTIMENSAT, &[AT_FDCWD, c.as_ptr() as u64, ptr, 0])
}
/// The flags statvfs reports for the filesystem holding `path` (`f_flags`).
fn statvfs_flags(path: &str) -> Result<u64, String> {
    let c = cpath(path);
    let mut buf = [0u64; 16];
    let r = sc(nr::STATFS, &[c.as_ptr() as u64, buf.as_mut_ptr() as u64]);
    if r < 0 { Err(format!("statfs {path} failed with {}", errname(-r))) } else { Ok(buf[10]) }
}
/// The flags fstatvfs reports for the filesystem holding `path`, through a descriptor
/// opened with `flags`.
fn fstatvfs_flags(path: &str, flags: u64) -> Result<u64, String> {
    let fd = ok(&format!("open {path}"), open(path, flags | O_CLOEXEC, 0))?;
    let mut buf = [0u64; 16];
    let r = sc(nr::FSTATFS, &[fd as u64, buf.as_mut_ptr() as u64]);
    close(fd);
    if r < 0 { Err(format!("fstatfs of {path} failed with {}", errname(-r))) } else { Ok(buf[10]) }
}
fn mknod(path: &str, mode: u32, dev: u64) -> i64 {
    let c = cpath(path);
    sc(nr::MKNODAT, &[AT_FDCWD, c.as_ptr() as u64, mode as u64, dev])
}
/// Map the first page of `path`, shared, through a descriptor opened with `flags`: the
/// address. The mapping stays until the process ends.
fn map_shared(path: &str, flags: u64, prot: u64) -> Result<u64, String> {
    let fd = ok(&format!("open {path}"), open(path, flags | O_CLOEXEC, 0))?;
    let addr = sc(nr::MMAP, &[0, 4096, prot, MAP_SHARED, fd as u64, 0]);
    close(fd);
    ok(&format!("mmap of {path}"), addr).map(|addr| addr as u64)
}
/// Store into the ELF identification's padding (byte 9), which a loader ignores.
fn store_padding(addr: u64) {
    // SAFETY: `addr` is a live, writable mapping of at least one page.
    unsafe { core::ptr::write_volatile((addr as *mut u8).add(9), 0x5a) };
}

/// Become `who` for good: its supplementary groups, then its group IDs, then its user
/// IDs, after which taking user ID 0 back must be refused.
fn become_account(who: Account) -> Checked {
    ok("setgroups", setgroups(who.groups))?;
    ok("setresgid", setresgid(who.gid, who.gid, who.gid))?;
    ok("setresuid", setresuid(who.uid, who.uid, who.uid))?;
    refused("setuid(0) after becoming the account", setuid(0), EPERM)
}

// ---------------------------------------------------------------------------
// Processes a case starts.

/// Read until end of file.
fn read_all(fd: Fd) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    while let Ok(n) = io::read(fd, &mut buf) {
        if n == 0 { break; }
        out.extend_from_slice(&buf[..n]);
    }
    out
}

/// Wait for `pid`; its wait status.
fn wait(pid: i32) -> Result<i32, String> {
    let mut status = 0;
    ok("waitpid", sc(nr::WAIT4, &[pid as i64 as u64, &mut status as *mut i32 as u64, 0, 0]))?;
    Ok(status)
}

fn status_text(s: i32) -> String {
    if s & 0x7f == 0 { format!("exit {}", (s >> 8) & 0xff) } else { format!("signal {}", s & 0x7f) }
}

/// Fork a child that becomes `who` (when given) and runs `f`, which reports a failure as
/// its message. Passes when the child reports no failure and exits 0.
fn as_account(who: Option<Account>, f: impl FnOnce() -> Checked) -> CaseResult {
    let (r, w) = io::pipe()?;
    match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(r);
            let result = who.map_or(Ok(()), become_account).and_then(|()| f());
            if let Err(msg) = &result {
                let _ = io::write(w, msg.as_bytes());
            }
            process::exit(if result.is_ok() { 0 } else { 1 })
        }
        ForkResult::Parent(child) => {
            let _ = io::close(w);
            let msg = read_all(r);
            let _ = io::close(r);
            let status = wait(child.raw() as i32)?;
            if !msg.is_empty() {
                return fail(String::from_utf8_lossy(&msg).into_owned());
            }
            check(status == 0, &format!("the child ended with {}", status_text(status)))
        }
    }
}

fn as_user(who: Account, f: impl FnOnce() -> Checked) -> CaseResult { as_account(Some(who), f) }

/// A process running as `who` that waits until it is killed or the case ends.
struct Holder { pid: i32, hold: Option<Fd> }

impl Holder {
    fn start(who: Account) -> Result<Holder, CaseError> {
        Holder::start_with(who, || Ok(String::new())).map(|(holder, _)| holder)
    }

    /// `start`, with `setup` run as `who` before the holder waits; what it returns is
    /// passed back, and its failure fails the case.
    fn start_with(who: Account, setup: impl FnOnce() -> Result<String, String>) -> Result<(Holder, String), CaseError> {
        Self::start_with_cleanup(who, setup, |_| Ok(()))
    }

    /// Run cleanup after the parent closes the hold pipe, before reporting exit.
    fn start_with_cleanup(
        who: Account,
        setup: impl FnOnce() -> Result<String, String>,
        cleanup: impl FnOnce(&str) -> Checked,
    ) -> Result<(Holder, String), CaseError> {
        let (ready_r, ready_w) = io::pipe()?;
        let (hold_r, hold_w) = io::pipe()?;
        match process::fork()? {
            ForkResult::Child => {
                let _ = io::close(ready_r);
                let _ = io::close(hold_w);
                let result = become_account(who).and_then(|()| setup());
                let report = match &result {
                    Ok(value) => format!("+{value}"),
                    Err(msg) => format!("-{msg}"),
                };
                let _ = io::write(ready_w, report.as_bytes());
                let _ = io::close(ready_w);
                read_all(hold_r);
                let cleaned = result.and_then(|value| cleanup(&value));
                process::exit(if cleaned.is_ok() { 0 } else { 1 })
            }
            ForkResult::Parent(child) => {
                let _ = io::close(ready_w);
                let _ = io::close(hold_r);
                let report = String::from_utf8_lossy(&read_all(ready_r)).into_owned();
                let _ = io::close(ready_r);
                let holder = Holder { pid: child.raw() as i32, hold: Some(hold_w) };
                match report.strip_prefix('+') {
                    Some(value) => Ok((holder, value.to_string())),
                    None => Err(CaseError::Fail(format!(
                        "the holder as {}: {}",
                        who.uid,
                        report.strip_prefix('-').unwrap_or("ended before reporting")
                    ))),
                }
            }
        }
    }

    /// Release the holder and require it to acknowledge successful cleanup.
    fn finish(mut self) -> CaseResult {
        if let Some(hold) = self.hold.take() { io::close(hold)?; }
        let status = wait(self.pid)?;
        self.pid = 0;
        check(status == 0, &format!("holder cleanup ended with {}", status_text(status)))
    }

    /// Whether the holder has not ended.
    fn alive(&self) -> bool {
        let mut status = 0;
        sc(nr::WAIT4, &[self.pid as i64 as u64, &mut status as *mut i32 as u64, WNOHANG as u64, 0]) == 0
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        if let Some(hold) = self.hold.take() { let _ = io::close(hold); }
        if self.pid != 0 {
            kill(self.pid, SIGKILL);
            let _ = wait(self.pid);
        }
    }
}

/// Run `f` in a new thread while the calling thread runs `main`; `f` waits for `go` and
/// returns what it saw.
fn with_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static, main: impl FnOnce()) -> Result<T, String> {
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let handle = std::thread::Builder::new()
        .spawn(move || { let _ = go_rx.recv(); f() })
        .map_err(|e| format!("thread spawn failed: {e}"))?;
    main();
    let _ = go_tx.send(());
    handle.join().map_err(|_| String::from("the thread panicked"))
}

// ---------------------------------------------------------------------------
// Files.

/// A directory for one case's files, removed with what it holds when the case ends.
struct Dir { path: String }

impl Dir {
    /// `/tmp/users-<pid>-<name>`, with `mode`, owned by `owner` and group `group`.
    fn new(name: &str, mode: u32, owner: u32, group: u32) -> Result<Dir, String> {
        Dir::at(&format!("/tmp/users-{}-{name}", pid()), mode, owner, group)
    }

    fn at(path: &str, mode: u32, owner: u32, group: u32) -> Result<Dir, String> {
        ok("mkdir", mkdir(path, 0o700))?;
        let dir = Dir { path: path.to_string() };
        ok("chown", chown(path, owner, group))?;
        ok("chmod", chmod(path, mode))?;
        Ok(dir)
    }

    fn path(&self, name: &str) -> String { format!("{}/{name}", self.path) }

    /// Create `name` holding `data`, owned by `uid`:`gid` with exactly `mode`.
    fn file(&self, name: &str, uid: u32, gid: u32, mode: u32, data: &[u8]) -> Result<String, String> {
        let path = self.path(name);
        make_file(&path, data)?;
        ok("chown", chown(&path, uid, gid))?;
        ok("chmod", chmod(&path, mode))?;
        Ok(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); let _ = rmdir(&self.path); }
}

/// Create `path` holding `data` as the caller, with mode 0600.
fn make_file(path: &str, data: &[u8]) -> Checked {
    let fd = ok(&format!("creating {path}"), open(path, O_CREAT | O_EXCL | O_WRONLY | O_CLOEXEC, 0o600))?;
    let written = write_fd(fd, data);
    close(fd);
    ok("write", written).map(|_| ())
}

/// The part of an ELF64 image a loader reads: up to the end of the furthest segment its
/// program headers name. Copying only this keeps the set-ID cases quick.
fn loadable(mut elf: Vec<u8>) -> Result<Vec<u8>, String> {
    let word = |elf: &[u8], at: usize, len: usize| -> Option<usize> {
        let bytes = elf.get(at..at + len)?;
        Some(bytes.iter().rev().fold(0usize, |v, &b| (v << 8) | b as usize))
    };
    let bad = || format!("{HELPER} is not an ELF64 image this case can read");
    if elf.get(..4) != Some(&b"\x7fELF"[..]) { return Err(bad()); }
    let phoff = word(&elf, 0x20, 8).ok_or_else(bad)?;
    let phentsize = word(&elf, 0x36, 2).ok_or_else(bad)?;
    let phnum = word(&elf, 0x38, 2).ok_or_else(bad)?;
    let mut end = phoff + phentsize * phnum;
    for i in 0..phnum {
        let header = phoff + i * phentsize;
        end = end.max(word(&elf, header + 8, 8).ok_or_else(bad)? + word(&elf, header + 32, 8).ok_or_else(bad)?);
    }
    if end > elf.len() { return Err(bad()); }
    elf.truncate(end);
    elf[0x28..0x30].fill(0);
    elf[0x3c..0x40].fill(0);
    Ok(elf)
}

/// A copy of the helper in `dir`, owned by `uid`:`gid` with `mode`.
fn helper_copy(dir: &Dir, name: &str, uid: u32, gid: u32, mode: u32) -> Result<String, String> {
    let bytes = loadable(std::fs::read(HELPER).map_err(|e| format!("reading {HELPER}: {e}"))?)?;
    dir.file(name, uid, gid, mode, &bytes)
}

/// Exec `path` with `args` in the calling (child) process; FD in `args` is replaced by
/// `report`. Returns only when the exec fails.
fn exec(path: &str, args: &[&str], report: Option<i64>) -> String {
    let path_c = cpath(path);
    let owned: Vec<CString> = args.iter()
        .map(|a| if *a == "FD" { cpath(&report.unwrap_or(-1).to_string()) } else { cpath(a) })
        .collect();
    let mut argv: Vec<*const u8> = owned.iter().map(|c| c.as_ptr() as *const u8).collect();
    argv.push(core::ptr::null());
    let envp: [*const u8; 1] = [core::ptr::null()];
    let r = sc(nr::EXECVE, &[path_c.as_ptr() as u64, argv.as_ptr() as u64, envp.as_ptr() as u64]);
    format!("exec {path} failed with {}", errname(-r))
}

/// Run the helper at `path` in a child: `setup` runs first (become an account, close
/// descriptors), then the exec with `args`, where FD is a pipe the helper reports on.
/// Returns the report as key/value pairs and the helper's wait status.
fn helper_report(
    path: &str,
    args: &[&str],
    setup: impl FnOnce() -> Checked,
) -> Result<(HashMap<String, String>, i32), CaseError> {
    let (r, w) = io::pipe()?;
    let (er, ew) = io::pipe()?;
    match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(r);
            let _ = io::close(er);
            let msg = match setup() {
                Ok(()) => {
                    // The error pipe closes on a successful exec.
                    mark_cloexec(ew.raw() as i64);
                    exec(path, args, Some(w.raw() as i64))
                }
                Err(msg) => msg,
            };
            let _ = io::write(ew, msg.as_bytes());
            process::exit(127)
        }
        ForkResult::Parent(child) => {
            let _ = io::close(w);
            let _ = io::close(ew);
            let failure = read_all(er);
            let out = read_all(r);
            let _ = io::close(er);
            let _ = io::close(r);
            let status = wait(child.raw() as i32)?;
            if !failure.is_empty() {
                return Err(CaseError::Fail(String::from_utf8_lossy(&failure).into_owned()));
            }
            if out.is_empty() && args.contains(&"FD") {
                return Err(CaseError::Fail(format!("{path} {} wrote no report and ended with {}", args[1], status_text(status))));
            }
            let map = String::from_utf8_lossy(&out).lines()
                .filter_map(|line| line.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect();
            Ok((map, status))
        }
    }
}

fn mark_cloexec(fd: i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    const FCNTL: u64 = 72;
    #[cfg(target_arch = "aarch64")]
    const FCNTL: u64 = 25;
    sc(FCNTL, &[fd as u64, 2, 1])
}

fn report_is(report: &HashMap<String, String>, key: &str, expected: &str, what: &str) -> Checked {
    match report.get(key) {
        Some(value) if value == expected => Ok(()),
        Some(value) => Err(format!("{what}: {key} is {value}, expected {expected}")),
        None => Err(format!("{what}: the program did not report {key}")),
    }
}

fn ids(uid: [u32; 3]) -> String { format!("{},{},{}", uid[0], uid[1], uid[2]) }

// ---------------------------------------------------------------------------
// credentials

fn ids_after_drop() -> CaseResult {
    as_user(A, || {
        want(resuid() == [UA; 3], format!("user IDs are {:?}, expected {UA} for each", resuid()))?;
        want(resgid() == [UA; 3], format!("group IDs are {:?}, expected {UA} for each", resgid()))?;
        let groups = getgroups()?;
        want(groups == [UG], format!("supplementary groups are {groups:?}, expected [{UG}]"))
    })
}

fn no_regain_root() -> CaseResult {
    as_user(A, || {
        refused("setuid(0)", setuid(0), EPERM)?;
        refused("setreuid(0, 0)", setreuid(0, 0), EPERM)?;
        refused("setreuid(-1, 0)", setreuid(KEEP, 0), EPERM)?;
        refused("setresuid(0, 0, 0)", setresuid(0, 0, 0), EPERM)?;
        refused("setresuid(-1, 0, -1)", setresuid(KEEP, 0, KEEP), EPERM)?;
        refused("setresgid(0, 0, 0)", setresgid(0, 0, 0), EPERM)?;
        want(resuid() == [UA; 3] && resgid() == [UA; 3], "a refused call changed the IDs")
    })
}

fn saved_toggle() -> CaseResult {
    as_account(None, || {
        ok("setgroups", setgroups(&[]))?;
        ok("setresgid", setresgid(UA, UA, UA))?;
        // Real ua, effective and saved ub: what a set-user-ID program owned by ub runs with.
        ok("setresuid", setresuid(UA, UB, UB))?;
        ok("setuid to the real user ID", setuid(UA))?;
        want(resuid() == [UA, UA, UB], format!("after setuid({UA}) the user IDs are {:?}", resuid()))?;
        ok("setuid back to the saved user ID", setuid(UB))?;
        want(resuid() == [UA, UB, UB], format!("after setuid({UB}) the user IDs are {:?}", resuid()))?;
        refused("setuid to an ID it never held", setuid(UW), EPERM)
    })
}

fn setres_rules() -> CaseResult {
    as_account(None, || {
        ok("setgroups", setgroups(&[]))?;
        ok("setresgid", setresgid(UA, UB, UW))?;
        ok("setresuid", setresuid(UA, UB, UW))?;
        ok("setresuid to a permutation of the current IDs", setresuid(UW, UA, UB))?;
        want(resuid() == [UW, UA, UB], format!("user IDs are {:?}, expected [{UW}, {UA}, {UB}]", resuid()))?;
        refused("setresuid to a new effective ID", setresuid(KEEP, 2004, KEEP), EPERM)?;
        refused("setresuid with one new ID among current ones", setresuid(UA, UB, 2004), EPERM)?;
        want(resuid() == [UW, UA, UB], "a refused setresuid changed the user IDs")?;
        ok("setresgid to a permutation of the current IDs", setresgid(UB, UW, UA))?;
        want(resgid() == [UB, UW, UA], format!("group IDs are {:?}, expected [{UB}, {UW}, {UA}]", resgid()))?;
        refused("setresgid to a new group", setresgid(KEEP, UG, KEEP), EPERM)?;
        want(resgid() == [UB, UW, UA], "a refused setresgid changed the group IDs")
    })
}

fn setgroups_user_eperm() -> CaseResult {
    as_user(A, || {
        refused("setgroups([wheel])", setgroups(&[WHEEL]), EPERM)?;
        refused("setgroups(0, NULL)", setgroups(&[]), EPERM)?;
        let groups = getgroups()?;
        want(groups == [UG], format!("groups are {groups:?} after the refusals, expected [{UG}]"))
    })
}

fn threads_share_groups() -> CaseResult {
    as_account(None, || {
        ok("setgroups", setgroups(&[UG]))?;
        let seen = with_thread(|| getgroups(), || { setgroups(&[WHEEL, UG]); })??;
        want(seen == [WHEEL, UG], format!("the other thread sees groups {seen:?}, expected [{WHEEL}, {UG}]"))?;
        // And the other way round: a thread's change reaches the first thread.
        let changed = with_thread(|| setgroups(&[UW]), || {})?;
        ok("setgroups in the second thread", changed)?;
        let groups = getgroups()?;
        want(groups == [UW], format!("the first thread sees groups {groups:?}, expected [{UW}]"))
    })
}

fn test_control(op: u64, arg: &mut [u64; 3]) -> i64 {
    let len = if op == TC_FIRED { 16 } else { 24 };
    sc(TEST_CONTROL, &[op, arg.as_mut_ptr() as u64, len])
}

fn orphan_thread_not_root() -> CaseResult {
    let dir = Dir::new("orphan", 0o755, 0, 0)?;
    // Root may read any file: a 0000 file owned by another user is the check's witness.
    let file = dir.file("secret", UB, UB, 0o000, b"secret")?;
    as_account(None, || {
        ok("open as root before the point is armed", try_open(&file, O_RDONLY))?;
        let mut fired = [CRED_ORPHAN as u64, 0, 0];
        ok("test_control FIRED", test_control(TC_FIRED, &mut fired))?;
        let before = fired[1];
        let mut arm = [CRED_ORPHAN as u64, gettid() as u64, 0];
        ok("test_control ARM cred-orphan", test_control(TC_ARM, &mut arm))?;
        refused("open by a thread whose process lookup fails", try_open(&file, O_RDONLY), ESRCH)?;
        ok("test_control FIRED", test_control(TC_FIRED, &mut fired))?;
        want(fired[1] == before + 1, format!("cred-orphan fired {} times, expected once", fired[1] - before))?;
        ok("open as root once the point has fired", try_open(&file, O_RDONLY)).map(|_| ())
    })
}

// ---------------------------------------------------------------------------
// files

fn other_read_eacces() -> CaseResult {
    let dir = Dir::new("other-read", 0o755, 0, 0)?;
    let file = dir.file("f", UB, UB, 0o600, b"b's")?;
    as_user(B, || ok("the owner's read", try_open(&file, O_RDONLY)).map(|_| ()))?;
    as_user(A, || refused("open for reading", try_open(&file, O_RDONLY), EACCES))
}

fn other_write_eacces() -> CaseResult {
    let dir = Dir::new("other-write", 0o755, 0, 0)?;
    let file = dir.file("f", UB, UB, 0o600, b"b's")?;
    as_user(B, || ok("the owner's write", try_open(&file, O_WRONLY)).map(|_| ()))?;
    as_user(A, || refused("open for writing", try_open(&file, O_WRONLY), EACCES))
}

fn group_member() -> CaseResult {
    let dir = Dir::new("group-member", 0o755, 0, 0)?;
    // ua's own group: the effective group ID selects the group class.
    let file = dir.file("f", UB, UA, 0o640, b"group")?;
    as_user(A, || {
        ok("open for reading as a member of the file's group", try_open(&file, O_RDONLY))?;
        refused("open for writing as a member of the file's group", try_open(&file, O_WRONLY), EACCES)
    })
}

fn group_nonmember() -> CaseResult {
    let dir = Dir::new("group-nonmember", 0o755, 0, 0)?;
    let file = dir.file("f", UB, UA, 0o640, b"group")?;
    as_user(W, || refused("open for reading outside the file's group", try_open(&file, O_RDONLY), EACCES))?;
    as_user(A, || ok("open for reading inside the file's group", try_open(&file, O_RDONLY)).map(|_| ()))
}

fn supplementary_access() -> CaseResult {
    let dir = Dir::new("supplementary", 0o755, 0, 0)?;
    let file = dir.file("f", UB, UG, 0o640, b"group")?;
    as_user(A, || ok("open for reading through supplementary group ug", try_open(&file, O_RDONLY)).map(|_| ()))?;
    as_user(W, || refused("open for reading without group ug", try_open(&file, O_RDONLY), EACCES))
}

fn root_exec_needs_x() -> CaseResult {
    let dir = Dir::new("root-exec", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", 0, 0, 0o644)?;
    as_account(None, || {
        let msg = exec(&program, &["p", "ran"], None);
        want(msg.contains("EACCES"), format!("root's exec of a 0644 file: {msg}"))
    })?;
    ok("chmod", chmod(&program, 0o614))?;
    let (_, status) = helper_report(&program, &["p", "ran"], || Ok(()))?;
    check(status == 77 << 8, &format!("root's exec of a file with group execute only ended with {}", status_text(status)))
}

fn search_denied() -> CaseResult {
    let dir = Dir::new("search", 0o755, 0, 0)?;
    let inner = dir.path("inner");
    ok("mkdir", mkdir(&inner, 0o755))?;
    let file = format!("{inner}/f");
    make_file(&file, b"x")?;
    ok("chmod", chmod(&file, 0o644))?;
    // Read and write but no search for others.
    ok("chmod", chmod(&inner, 0o776))?;
    as_user(A, || {
        refused("open through a directory without search permission", try_open(&file, O_RDONLY), EACCES)?;
        refused("chdir into it", chdir(&inner), EACCES)
    })?;
    ok("chmod", chmod(&inner, 0o777))?;
    as_user(A, || ok("open once the directory may be searched", try_open(&file, O_RDONLY)).map(|_| ()))
}

fn create_owner() -> CaseResult {
    let dir = Dir::new("create", 0o777, 0, 0)?;
    let file = dir.path("mine");
    as_user(A, || {
        umask(0o022);
        let fd = ok("create", open(&file, O_CREAT | O_EXCL | O_WRONLY, 0o666))?;
        close(fd);
        let st = stat(&file)?;
        want((st.st_uid, st.st_gid) == (UA, UA), format!("the new file is {}:{}, expected {UA}:{UA}", st.st_uid, st.st_gid))?;
        want(st.st_mode & 0o7777 == 0o644, format!("the new file's mode is {:o}, expected 644", st.st_mode & 0o7777))
    })
}

fn sgid_dir_group() -> CaseResult {
    let dir = Dir::new("sgid", 0o2777, 0, UG)?;
    let (file, sub) = (dir.path("f"), dir.path("d"));
    as_user(B, || {
        umask(0);
        make_file(&file, b"x")?;
        ok("mkdir", mkdir(&sub, 0o755))?;
        let st = stat(&file)?;
        want(st.st_gid == UG, format!("the new file's group is {}, expected the directory's {UG}", st.st_gid))?;
        let st = stat(&sub)?;
        want(st.st_gid == UG, format!("the new directory's group is {}, expected {UG}", st.st_gid))?;
        want(st.st_mode & 0o2000 != 0, format!("the new directory's mode is {:o}, without set-group-ID", st.st_mode & 0o7777))
    })
}

fn chmod_not_owner() -> CaseResult {
    let dir = Dir::new("chmod", 0o755, 0, 0)?;
    let file = dir.file("f", UB, UB, 0o644, b"x")?;
    as_user(A, || refused("chmod of ub's file", chmod(&file, 0o666), EPERM))?;
    check(mode_of(&file)? & 0o7777 == 0o644, "a refused chmod changed the mode")?;
    as_user(B, || ok("chmod by the owner", chmod(&file, 0o600)).map(|_| ()))
}

fn chown_giveaway() -> CaseResult {
    let dir = Dir::new("chown", 0o755, 0, 0)?;
    let file = dir.file("f", UA, UA, 0o644, b"x")?;
    as_user(A, || refused("chown of ua's own file to ub", chown(&file, UB, KEEP), EPERM))?;
    check(stat(&file)?.st_uid == UA, "a refused chown changed the owner")
}

fn chgrp_nonmember() -> CaseResult {
    let dir = Dir::new("chgrp", 0o755, 0, 0)?;
    let file = dir.file("f", UA, UA, 0o644, b"x")?;
    as_user(A, || {
        refused("chgrp to a group ua is not in", chown(&file, KEEP, UB), EPERM)?;
        ok("chgrp to ua's supplementary group", chown(&file, KEEP, UG))?;
        want(stat(&file)?.st_gid == UG, "chgrp to ua's group did not take")
    })
}

fn write_clears_setid() -> CaseResult {
    let dir = Dir::new("setid-write", 0o755, 0, 0)?;
    let written = dir.file("w", UB, UB, 0o6777, b"x")?;
    let truncated = dir.file("t", UB, UB, 0o6777, b"x")?;
    as_user(A, || {
        let fd = ok("open for writing", open(&written, O_WRONLY, 0))?;
        let n = write_fd(fd, b"y");
        close(fd);
        ok("write", n)?;
        let fd = ok("open for writing", open(&truncated, O_WRONLY, 0))?;
        let r = sc(nr::FTRUNCATE, &[fd as u64, 0]);
        close(fd);
        ok("ftruncate", r).map(|_| ())
    })?;
    for (path, what) in [(&written, "write"), (&truncated, "truncate")] {
        let mode = mode_of(path)? & 0o7777;
        check(mode == 0o777, &format!("after ua's {what} the mode is {mode:o}, expected 777"))?;
    }
    Ok(())
}

fn mmap_clears_setid() -> CaseResult {
    fn unmap(addr: u64) -> Checked {
        ok("munmap of shared file", sc(libbreenix::syscall::nr::MUNMAP, &[addr, 4096])).map(|_| ())
    }
    fn unmap_report(value: &str) -> Checked {
        unmap(value.parse().map_err(|_| String::from("invalid mapped address"))?)
    }
    let dir = Dir::new("setid-mmap", 0o755, 0, 0)?;
    // A root-owned set-ID program, mode 6777, that ua may write.
    let program = helper_copy(&dir, "p", 0, 0, 0o6777)?;
    let setid_mode = |what: &str| -> Checked {
        let mode = mode_of(&program)? & 0o7777;
        want(mode == 0o6777, format!("{what}: the mode is {mode:o}, expected 6777"))
    };
    let cleared = |what: &str| -> Checked {
        let mode = mode_of(&program)? & 0o7777;
        want(mode == 0o777, format!("{what}: the mode is {mode:o}, expected 777"))
    };
    // Root sets the bits again once earlier stores are on disk, whose writeback would
    // otherwise clear them under the next check.
    let reset = || -> Checked {
        let fd = ok("open as root", open(&program, O_RDONLY | O_CLOEXEC, 0))?;
        let synced = sc(nr::FSYNC, &[fd as u64]);
        close(fd);
        ok("fsync as root", synced)?;
        ok("chmod 6777 as root", chmod(&program, 0o6777)).map(|_| ())
    };
    as_user(A, || {
        let addr = map_shared(&program, O_RDONLY, PROT_READ)?;
        setid_mode("after ua's read-only shared mapping")?;
        unmap(addr)?;
        let addr = map_shared(&program, O_RDONLY, PROT_READ)?;
        refused("mprotect(PROT_WRITE) of a mapping opened read-only", sc(nr::MPROTECT, &[addr, 4096, PROT_READ | PROT_WRITE]), EACCES)?;
        setid_mode("after ua's refused mprotect")?;
        unmap(addr)?;
        let addr = map_shared(&program, O_RDWR, PROT_WRITE | PROT_READ)?;
        cleared("after ua's shared writable mapping")?;
        store_padding(addr);
        cleared("after ua's store through the mapping")?;
        unmap(addr)
    })?;
    reset()?;
    as_user(A, || {
        let addr = map_shared(&program, O_RDWR, PROT_READ)?;
        setid_mode("after ua's read-only mapping of a writable descriptor")?;
        ok("mprotect(PROT_WRITE)", sc(nr::MPROTECT, &[addr, 4096, PROT_READ | PROT_WRITE]))?;
        cleared("after ua's mprotect to PROT_WRITE")?;
        unmap(addr)
    })?;
    reset()?;
    // ua's stores wait for writeback while root sets the bits again: exec of the file
    // must confer no identity, as the stores' writeback will clear the bits.
    let (writer, _) = Holder::start_with_cleanup(A, || {
        let addr = map_shared(&program, O_RDWR, PROT_READ | PROT_WRITE)?;
        store_padding(addr);
        Ok(addr.to_string())
    }, unmap_report)?;
    ok("chmod 6777 as root", chmod(&program, 0o6777))?;
    let (report, _) = helper_report(&program, &["p", "ids", "FD"], || become_account(B))?;
    report_is(&report, "resuid", &ids([UB; 3]), "a set-user-ID program with mapped stores pending")?;
    report_is(&report, "resgid", &ids([UB; 3]), "a set-group-ID program with mapped stores pending")?;
    writer.finish()?;
    // These mappings are explicitly unmapped before the final positive check:
    // wait4 alone need not finish the deferred page-table/binding retirement.
    // No first write fault has occurred when root restores the bits. Exec must
    // already decline privilege: a first fault can change the ELF during its
    // copy without taking the filesystem guard. Include a read-only binding
    // that can gain PROT_WRITE by mprotect while exec holds that guard.
    for prot in [PROT_READ | PROT_WRITE, PROT_READ] {
        reset()?;
        let (writer, _) = Holder::start_with_cleanup(A, || {
            map_shared(&program, O_RDWR, prot).map(|addr| addr.to_string())
        }, unmap_report)?;
        ok("chmod 6777 over a clean mapping as root", chmod(&program, 0o6777))?;
        let (report, _) = helper_report(&program, &["p", "ids", "FD"], || become_account(B))?;
        report_is(&report, "resuid", &ids([UB; 3]), "a set-ID program with a clean shared write-capable mapping")?;
        report_is(&report, "resgid", &ids([UB; 3]), "a set-ID program with a clean shared write-capable mapping")?;
        writer.finish()?;
    }
    reset()?;
    let (report, _) = helper_report(&program, &["p", "ids", "FD"], || become_account(B))?;
    report_is(&report, "resuid", &ids([UB, 0, 0]), "the program after the shared writer unmapped")?;
    report_is(&report, "resgid", &ids([UB, 0, 0]), "the program after the shared writer unmapped").map_err(CaseError::from)
}

fn utimes_nonowner() -> CaseResult {
    let dir = Dir::new("utimes", 0o755, 0, 0)?;
    let writable = dir.file("w", UB, UB, 0o666, b"x")?;
    let readonly = dir.file("r", UB, UB, 0o644, b"x")?;
    as_user(A, || {
        refused("explicit times on ub's writable file", utimens(&writable, Some([1000, 0, 2000, 0])), EPERM)?;
        refused("now and an explicit time on ub's writable file", utimens(&writable, Some([0, UTIME_NOW, 2000, 0])), EPERM)?;
        ok("both times now on ub's writable file", utimens(&writable, None))?;
        ok("both UTIME_NOW on ub's writable file", utimens(&writable, Some([0, UTIME_NOW, 0, UTIME_NOW])))?;
        refused("both times now on ub's read-only file", utimens(&readonly, None), EACCES)
    })?;
    as_user(B, || ok("explicit times by the owner", utimens(&readonly, Some([1000, 0, 2000, 0]))).map(|_| ()))
}

// ---------------------------------------------------------------------------
// sticky

fn tmp_1777() -> CaseResult {
    let st = stat("/tmp")?;
    check(st.st_mode == 0o041777, &format!("/tmp's mode is {:o}, expected 41777", st.st_mode))?;
    check((st.st_uid, st.st_gid) == (0, 0), &format!("/tmp is {}:{}, expected 0:0", st.st_uid, st.st_gid))
}

/// A root-owned sticky world-writable directory with a file `b` that ub made.
fn sticky_with_b(name: &str) -> Result<(Dir, String), CaseError> {
    let dir = Dir::new(name, 0o1777, 0, 0)?;
    let file = dir.path("b");
    as_user(B, || make_file(&file, b"b's"))?;
    Ok((dir, file))
}

fn sticky_unlink() -> CaseResult {
    let (dir, file) = sticky_with_b("sticky-unlink")?;
    as_user(A, || refused("unlink of ub's file", unlink(&file), EPERM))?;
    stat(&file)?;
    // Without the sticky bit the same unlink is allowed: the bit is what refuses it.
    ok("chmod", chmod(&dir.path, 0o777))?;
    as_user(A, || ok("unlink without the sticky bit", unlink(&file)).map(|_| ()))
}

fn sticky_rename() -> CaseResult {
    let (dir, file) = sticky_with_b("sticky-rename")?;
    let to = dir.path("taken");
    as_user(A, || refused("rename of ub's file", rename(&file, &to), EPERM))?;
    stat(&file)?;
    as_user(B, || ok("rename by the file's owner", rename(&file, &to)).map(|_| ()))
}

fn sticky_replace() -> CaseResult {
    let (dir, file) = sticky_with_b("sticky-replace")?;
    let mine = dir.path("a");
    as_user(A, || {
        make_file(&mine, b"a's")?;
        refused("rename over ub's file", rename(&mine, &file), EPERM)
    })?;
    check(stat(&file)?.st_uid == UB, "ub's file was replaced")
}

fn sticky_dir_owner() -> CaseResult {
    let dir = Dir::new("sticky-owner", 0o1777, UW, UW)?;
    let file = dir.path("b");
    as_user(B, || make_file(&file, b"b's"))?;
    as_user(A, || refused("unlink by a third user", unlink(&file), EPERM))?;
    as_user(W, || ok("unlink by the directory's owner", unlink(&file)).map(|_| ()))
}

fn sticky_symlink() -> CaseResult {
    let dir = Dir::new("sticky-link", 0o1777, 0, 0)?;
    let target = dir.file("target", 0, 0, 0o644, b"target")?;
    let link_path = dir.path("link");
    as_user(B, || ok("symlink", symlink(&target, &link_path)).map(|_| ()))?;
    as_user(A, || refused("open through ub's link", try_open(&link_path, O_RDONLY), EACCES))?;
    as_account(None, || refused("open through ub's link as root", try_open(&link_path, O_RDONLY), EACCES))?;
    as_user(B, || ok("open through the link by its owner", try_open(&link_path, O_RDONLY)).map(|_| ()))?;
    as_user(A, || ok("open of the target by name", try_open(&target, O_RDONLY)).map(|_| ()))
}

fn sticky_hardlink() -> CaseResult {
    let dir = Dir::new("sticky-hardlink", 0o1777, 0, 0)?;
    let private = dir.file("private", UB, UB, 0o600, b"b's")?;
    let shared = dir.file("shared", UB, UB, 0o666, b"b's")?;
    let (mine, also) = (dir.path("mine"), dir.path("also"));
    as_user(A, || {
        refused("link to ub's 0600 file", link(&private, &mine), EPERM)?;
        ok("link to ub's 0666 file, which ua may read and write", link(&shared, &also)).map(|_| ())
    })
}

// ---------------------------------------------------------------------------
// setid-exec

fn setuid_euid() -> CaseResult {
    let dir = Dir::new("setuid", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UB, UB, 0o4755)?;
    let (report, _) = helper_report(&program, &["p", "ids", "FD"], || become_account(A))?;
    report_is(&report, "resuid", &ids([UA, UB, UB]), "a set-user-ID program owned by ub run by ua")?;
    report_is(&report, "resgid", &ids([UA; 3]), "a set-user-ID program owned by ub run by ua").map_err(CaseError::from)
}

fn setuid_saved() -> CaseResult {
    let dir = Dir::new("setuid-saved", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UB, UB, 0o4755)?;
    let (report, status) = helper_report(&program, &["p", "saved", "FD"], || become_account(A))?;
    report_is(&report, "resuid", &ids([UA, UB, UB]), "the set-user-ID program")?;
    let step = match (status >> 8) & 0xff {
        0 if status & 0x7f == 0 => return Ok(()),
        10 => "setuid to the real user ID failed",
        11 => "setuid to the real user ID left the effective ID unchanged",
        12 => "setuid back to the saved user ID failed",
        13 => "setuid back to the saved user ID left the effective ID unchanged",
        _ => return fail(format!("the set-user-ID program ended with {}", status_text(status))),
    };
    fail(step)
}

fn setgid_egid() -> CaseResult {
    let dir = Dir::new("setgid", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UB, UG, 0o2755)?;
    let (report, _) = helper_report(&program, &["p", "ids", "FD"], || become_account(B))?;
    report_is(&report, "resgid", &ids([UB, UG, UG]), "a set-group-ID program of group ug run by ub")?;
    report_is(&report, "resuid", &ids([UB; 3]), "a set-group-ID program run by ub").map_err(CaseError::from)
}

fn setgid_no_gx() -> CaseResult {
    let dir = Dir::new("setgid-nogx", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UA, UG, 0o2745)?;
    let (report, _) = helper_report(&program, &["p", "ids", "FD"], || become_account(B))?;
    report_is(&report, "resgid", &ids([UB; 3]), "a set-group-ID program without group execute").map_err(CaseError::from)
}

fn nosuid_home() -> CaseResult {
    let flags = statvfs_flags("/home")?;
    check(flags & (ST_NOSUID | ST_NODEV) == ST_NOSUID | ST_NODEV,
        &format!("statvfs(/home) flags are {flags:#x}, without ST_NOSUID and ST_NODEV"))?;
    let fd_flags = fstatvfs_flags("/home", O_RDONLY)?;
    check(fd_flags == flags, &format!("fstatvfs of a descriptor for /home reports {fd_flags:#x}, statvfs {flags:#x}"))?;
    let mounts = std::fs::read_to_string("/proc/mounts").map_err(|e| format!("reading /proc/mounts: {e}"))?;
    let home = mounts.lines().find(|line| line.split_whitespace().nth(1) == Some("/home"));
    check(home.is_some_and(|line| {
        let options: Vec<&str> = line.split_whitespace().nth(3).unwrap_or("").split(',').collect();
        options.contains(&"nosuid") && options.contains(&"nodev")
    }), &format!("/proc/mounts shows /home as {home:?}"))?;
    let dir = Dir::at(&format!("/home/users-{}", pid()), 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UB, UB, 0o6755)?;
    let file_flags = fstatvfs_flags(&program, O_RDONLY)?;
    check(file_flags == flags, &format!("fstatvfs of a file under /home reports {file_flags:#x}, statvfs {flags:#x}"))?;
    let (report, _) = helper_report(&program, &["p", "ids", "FD"], || become_account(A))?;
    report_is(&report, "resuid", &ids([UA; 3]), "a set-user-ID program under /home")?;
    report_is(&report, "resgid", &ids([UA; 3]), "a set-group-ID program under /home")?;
    report_is(&report, "secure", "0", "a set-ID program under /home").map_err(CaseError::from)
}

fn shared_fs_no_gain() -> CaseResult {
    let dir = Dir::new("shared-fs", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UB, UB, 0o4755)?;
    // The sharer reads this pipe until its two write ends are gone: the case's, and the
    // exec'd program's, which ends when the program does.
    let (hold_r, hold_w) = io::pipe()?;
    let result = helper_report(&program, &["p", "ids", "FD"], || {
        become_account(A)?;
        // A second process sharing the working directory, as clone(CLONE_FS) makes it,
        // that outlives the exec.
        let r = sc(nr::CLONE, &[CLONE_FS | SIGCHLD, 0, 0, 0, 0]);
        if r == 0 {
            // Keep only the hold pipe, so the case's report pipes end with the exec'd program.
            for fd in 0..64 {
                if fd != hold_r.raw() as i64 { close(fd); }
            }
            read_all(hold_r);
            process::exit(0);
        }
        ok("clone(CLONE_FS)", r).map(|_| ())
    });
    let _ = io::close(hold_w);
    let _ = io::close(hold_r);
    let (report, _) = result?;
    report_is(&report, "resuid", &ids([UA; 3]), "a set-user-ID exec while another process shares the working directory")
        .map_err(CaseError::from)
}

fn at_secure() -> CaseResult {
    let dir = Dir::new("at-secure", 0o755, 0, 0)?;
    let setuid = helper_copy(&dir, "s", UB, UB, 0o4755)?;
    let plain = helper_copy(&dir, "p", UB, UB, 0o755)?;
    let (report, _) = helper_report(&plain, &["p", "ids", "FD"], || become_account(A))?;
    report_is(&report, "secure", "0", "an ordinary program")?;
    let (report, _) = helper_report(&setuid, &["s", "ids", "FD"], || become_account(A))?;
    report_is(&report, "secure", "1", "a set-user-ID program").map_err(CaseError::from)
}

fn closed_stdio() -> CaseResult {
    let dir = Dir::new("stdio", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UB, UB, 0o4755)?;
    let close_stdio = || {
        become_account(A)?;
        for fd in 0..3 { close(fd); }
        Ok(())
    };
    let (report, _) = helper_report(&program, &["p", "ids", "FD"], close_stdio)?;
    report_is(&report, "resuid", &ids([UA, UB, UB]), "the set-user-ID program")?;
    for fd in ["fd0", "fd1", "fd2"] {
        report_is(&report, fd, "null", "a set-ID program started with descriptors 0 to 2 closed")?;
    }
    // An ordinary program keeps them closed: the kernel opens them only for a set-ID exec.
    let plain = helper_copy(&dir, "o", UB, UB, 0o755)?;
    let (report, _) = helper_report(&plain, &["o", "ids", "FD"], close_stdio)?;
    report_is(&report, "fd1", "closed", "an ordinary program started with descriptor 1 closed").map_err(CaseError::from)
}

// ---------------------------------------------------------------------------
// signals

fn kill_other() -> CaseResult {
    let target = Holder::start(B)?;
    let pid = target.pid;
    as_user(A, || refused("kill(SIGTERM) of ub's process", kill(pid, SIGTERM), EPERM))?;
    check(target.alive(), "ub's process ended after the refused kill")
}

fn kill_probe() -> CaseResult {
    let target = Holder::start(B)?;
    let pid = target.pid;
    as_user(A, || refused("kill(pid, 0) of ub's process", kill(pid, 0), EPERM))?;
    as_user(B, || ok("kill(pid, 0) by its owner", kill(pid, 0)).map(|_| ()))
}

fn kill_setuid_child() -> CaseResult {
    let dir = Dir::new("kill-setuid", 0o755, 0, 0)?;
    let program = helper_copy(&dir, "p", UB, UB, 0o4755)?;
    as_user(A, || {
        let (ready_r, ready_w) = io::pipe().map_err(|e| e.to_string())?;
        let (hold_r, hold_w) = io::pipe().map_err(|e| e.to_string())?;
        let child = match process::fork().map_err(|e| e.to_string())? {
            ForkResult::Child => {
                let _ = io::close(ready_r);
                let _ = io::close(hold_w);
                let ready = ready_w.raw().to_string();
                let hold = hold_r.raw().to_string();
                exec(&program, &["p", "hold", &ready, &hold], None);
                process::exit(127)
            }
            ForkResult::Parent(child) => child.raw() as i32,
        };
        let _ = io::close(ready_w);
        let _ = io::close(hold_r);
        let report = String::from_utf8_lossy(&read_all(ready_r)).into_owned();
        let _ = io::close(ready_r);
        let sent = kill(child, SIGTERM);
        let _ = io::close(hold_w);
        let status = wait(child)?;
        want(report.lines().any(|l| l == format!("resuid={}", ids([UA, UB, UB]))),
            format!("the set-user-ID child did not report running as ub: {report:?}"))?;
        ok("kill of the set-user-ID child ua started", sent)?;
        want(status & 0x7f == SIGTERM, format!("the child ended with {}, expected signal {SIGTERM}", status_text(status)))
    })
}

fn root_kills_any() -> CaseResult {
    let target = Holder::start(B)?;
    let pid = target.pid;
    ok("kill(SIGKILL) as root", kill(pid, SIGKILL))?;
    let status = wait(pid)?;
    core::mem::forget(target);
    check(status & 0x7f == SIGKILL, &format!("ub's process ended with {}, expected signal {SIGKILL}", status_text(status)))
}

fn renice_other() -> CaseResult {
    let target = Holder::start(B)?;
    let pid = target.pid;
    as_user(A, || refused("setpriority of ub's process", sc(nr::SETPRIORITY, &[PRIO_PROCESS, pid as u64, 5]), EPERM))?;
    as_user(B, || ok("setpriority by its owner", sc(nr::SETPRIORITY, &[PRIO_PROCESS, pid as u64, 5])).map(|_| ()))
}

// ---------------------------------------------------------------------------
// proc-dev

fn proc_exe_other() -> CaseResult {
    let target = Holder::start(B)?;
    let path = format!("/proc/{}/exe", target.pid);
    as_user(A, || {
        match readlink(&path) {
            Err(r) => refused(&format!("readlink {path}"), r, EACCES)?,
            Ok(target) => return Err(format!("readlink {path} gave {target}, expected EACCES")),
        }
        readlink("/proc/self/exe").map(|_| ()).map_err(|r| format!("readlink /proc/self/exe failed with {}", shown(r)))
    })
}

/// The thread IDs `/proc/<pid>/task` lists.
fn task_ids(pid: i32) -> Result<Vec<String>, String> {
    let dir = format!("/proc/{pid}/task");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("reading {dir}: {e}"))?;
    Ok(entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
}

fn kernel_output_other() -> CaseResult {
    let target = Holder::start(B)?;
    let tid = task_ids(target.pid)?.into_iter().next().ok_or("ub's process lists no thread")?;
    let path = format!("/proc/{}/task/{tid}/kernel_output", target.pid);
    ok(&format!("open {path} as root"), try_open(&path, O_RDONLY))?;
    as_user(A, || {
        refused(&format!("open {path}"), try_open(&path, O_RDONLY), EACCES)?;
        let me = pid();
        let own = format!("/proc/{me}/task/{}/kernel_output", gettid());
        ok("open of the caller's own kernel_output", try_open(&own, O_RDONLY)).map(|_| ())
    })
}

fn kmsg_user() -> CaseResult {
    ok("open /proc/kmsg as root", try_open("/proc/kmsg", O_RDONLY))?;
    as_user(A, || refused("open /proc/kmsg", try_open("/proc/kmsg", O_RDONLY), EACCES))
}

fn trace_user() -> CaseResult {
    ok("open /proc/trace/counters as root", try_open("/proc/trace/counters", O_RDONLY))?;
    as_user(A, || {
        for path in ["/proc/trace/counters", "/proc/trace/buffer", "/proc/trace/enable"] {
            refused(&format!("open {path}"), try_open(path, O_RDONLY), EACCES)?;
        }
        Ok(())
    })
}

fn vcs_user() -> CaseResult {
    for (path, mode) in [("/dev/vcs1", 0o020600), ("/dev/console", 0o020600), ("/dev/null", 0o020666)] {
        let st = stat(path)?;
        check(st.st_mode == mode && st.st_uid == 0,
            &format!("{path} is mode {:o} owner {}, expected {mode:o} owner 0", st.st_mode, st.st_uid))?;
    }
    ok("open /dev/vcs1 as root", try_open("/dev/vcs1", O_RDONLY))?;
    as_user(A, || {
        refused("open /dev/vcs1", try_open("/dev/vcs1", O_RDONLY), EACCES)?;
        refused("open /dev/vcsa1", try_open("/dev/vcsa1", O_RDONLY), EACCES)?;
        refused("open /dev/console for writing", try_open("/dev/console", O_WRONLY), EACCES)?;
        ok("open /dev/null for reading and writing", try_open("/dev/null", O_RDWR)).map(|_| ())
    })
}

/// Bind a new TCP socket to port `port` on any address: the raw return.
fn bind_tcp(port: u16) -> i64 {
    let fd = sc(nr::SOCKET, &[2, 1, 0]);
    if fd < 0 { return fd; }
    let mut addr = [0u8; 16];
    addr[0] = 2;
    addr[2..4].copy_from_slice(&port.to_be_bytes());
    let r = sc(nr::BIND, &[fd as u64, addr.as_ptr() as u64, addr.len() as u64]);
    close(fd);
    r
}

fn tcp_low_port() -> CaseResult {
    as_user(A, || {
        refused("bind of TCP port 1013", bind_tcp(1013), EACCES)?;
        ok("bind of TCP port 41013", bind_tcp(41013)).map(|_| ())
    })?;
    as_account(None, || ok("bind of TCP port 1013 as root", bind_tcp(1013)).map(|_| ()))
}

fn home_nodev_device() -> CaseResult {
    let home = Dir::at(&format!("/home/users-{}-dev", pid()), 0o755, 0, 0)?;
    let tmp = Dir::new("dev", 0o777, 0, 0)?;
    let (on_home, on_tmp) = (home.path("null"), tmp.path("null"));
    ok("mknod of a null device under /home as root", mknod(&on_home, S_IFCHR | 0o666, NULL_RDEV))?;
    ok("mknod of a null device under /tmp as root", mknod(&on_tmp, S_IFCHR | 0o666, NULL_RDEV))?;
    // The same node opens on the root filesystem: what refuses it under /home is nodev.
    ok("open of the null device under /tmp", try_open(&on_tmp, O_RDWR))?;
    refused("open of the null device under /home as root", try_open(&on_home, O_RDWR), EACCES)?;
    as_user(A, || {
        refused("open of the null device under /home", try_open(&on_home, O_RDWR), EACCES)?;
        refused("mknod of a device", mknod(&tmp.path("mine"), S_IFCHR | 0o666, NULL_RDEV), EPERM)
    })
}

/// An FBDRAW command: the raw return.
fn fbdraw(op: u32, p: [u64; 4], color: u32) -> i64 {
    let cmd = libbreenix::graphics::FbDrawCmd { op, p1: p[0] as i32, p2: p[1] as i32, p3: p[2] as i32, p4: p[3] as i32, color };
    sc(FBDRAW, &[&cmd as *const _ as u64])
}
fn lo_hi(ptr: u64) -> [u64; 2] { [ptr & 0xffff_ffff, ptr >> 32] }

/// Create and register a 4x4 window titled `title`: its buffer ID.
#[cfg(target_arch = "aarch64")]
fn make_window(title: &str) -> Result<u32, String> {
    let mut addr = 0u64;
    let [lo, hi] = lo_hi(&mut addr as *mut u64 as u64);
    let id = ok("creating a window", fbdraw(11, [4, 4, lo, hi], 0))?;
    let [lo, hi] = lo_hi(title.as_ptr() as u64);
    ok("registering the window", fbdraw(12, [id as u64, lo, hi, title.len() as u64], 0))?;
    Ok(id as u32)
}

/// The buffer IDs of the windows the caller may list.
#[cfg(target_arch = "aarch64")]
fn listed_windows() -> Result<Vec<u32>, String> {
    let mut out = [libbreenix::graphics::WindowInfo::default(); 16];
    let [lo, hi] = lo_hi(out.as_mut_ptr() as u64);
    let n = ok("listing windows", fbdraw(13, [lo, hi, out.len() as u64, 0], 0))?;
    Ok(out[..n as usize].iter().map(|w| w.buffer_id).collect())
}

/// Each window operation a process may perform only on its own windows, on `id`: what
/// it is and the raw return.
fn window_ops(id: u32) -> Vec<(&'static str, i64)> {
    let id = id as u64;
    let mut pixels = [0u8; 64];
    let mut events = [libbreenix::graphics::WindowInputEvent::default(); 4];
    let event = libbreenix::graphics::WindowInputEvent::default();
    let mut mapped = 0u64;
    let [plo, phi] = lo_hi(pixels.as_mut_ptr() as u64);
    let [elo, ehi] = lo_hi(events.as_mut_ptr() as u64);
    let [ilo, ihi] = lo_hi(&event as *const _ as u64);
    let [mlo, mhi] = lo_hi(&mut mapped as *mut u64 as u64);
    vec![
        ("reading its pixels", fbdraw(14, [id, plo, phi, pixels.len() as u64], 0)),
        ("reading its input", fbdraw(19, [id, elo, ehi, events.len() as u64], 1)),
        ("writing its input", fbdraw(18, [id, ilo, ihi, 0], 0)),
        ("mapping its pixels", fbdraw(21, [id, mlo, mhi, 0], 0)),
        ("checking it for a new frame", fbdraw(22, [id, 0, 0, 0], 0)),
        ("moving it", fbdraw(17, [id, 0, 0, 0], 0)),
    ]
}

#[cfg(target_arch = "aarch64")]
fn window_other() -> CaseResult {
    let (_owner, id) = Holder::start_with(B, || make_window("ub's").map(|id| id.to_string()))?;
    let id: u32 = id.parse().map_err(|_| format!("ub's window ID {id:?} is not a number"))?;
    check(listed_windows()?.contains(&id), "root's window list does not show ub's window")?;
    as_user(A, || {
        refused("taking over the display as a non-root user", sc(libbreenix::syscall::nr::TAKE_OVER_DISPLAY, &[]), EPERM)?;
        let listed = listed_windows()?;
        want(!listed.contains(&id), format!("ua's window list shows ub's window {id}: {listed:?}"))?;
        for (what, ret) in window_ops(id) {
            refused(&format!("{what} of ub's window"), ret, EPERM)?;
        }
        let mine = make_window("ua's")?;
        want(listed_windows()? == [mine], "ua's window list does not show exactly ua's own window")?;
        for (what, ret) in window_ops(mine) {
            ok(&format!("{what} of ua's own window"), ret)?;
        }
        Ok(())
    })
}

/// x86-64 has no window registry: its framebuffer call refuses the window operations (EINVAL).
#[cfg(target_arch = "x86_64")]
fn window_other() -> CaseResult {
    as_user(A, || {
        refused("taking over the display as a non-root user", sc(libbreenix::syscall::nr::TAKE_OVER_DISPLAY, &[]), EPERM)?;
        refused("creating a window", fbdraw(11, [4, 4, 0, 0], 0), EINVAL)?;
        refused("listing windows", fbdraw(13, [0, 0, 0, 0], 0), EINVAL)?;
        for (what, ret) in window_ops(1) {
            refused(&format!("{what} of window 1"), ret, EINVAL)?;
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// umask

fn umask_file() -> CaseResult {
    let dir = Dir::new("umask-file", 0o777, 0, 0)?;
    let file = dir.path("f");
    as_user(A, || {
        umask(0o027);
        let fd = ok("create", open(&file, O_CREAT | O_EXCL | O_WRONLY, 0o666))?;
        close(fd);
        let mode = mode_of(&file)? & 0o7777;
        want(mode == 0o640, format!("a file created with 666 under umask 027 has mode {mode:o}, expected 640"))
    })
}

fn umask_dir() -> CaseResult {
    let dir = Dir::new("umask-dir", 0o777, 0, 0)?;
    let sub = dir.path("d");
    as_user(A, || {
        umask(0o072);
        ok("mkdir", mkdir(&sub, 0o777))?;
        let mode = mode_of(&sub)? & 0o7777;
        want(mode == 0o705, format!("a directory made with 777 under umask 072 has mode {mode:o}, expected 705"))
    })
}

fn umask_fork() -> CaseResult {
    as_user(A, || {
        umask(0o035);
        as_account(None, || {
            let seen = umask(0o022);
            want(seen == 0o035, format!("the forked child's umask is {seen:o}, expected 35"))
        }).map_err(|e| match e { CaseError::Fail(msg) => msg, _ => String::from("the forked child failed") })?;
        let kept = umask(0o035);
        want(kept == 0o035, format!("the parent's umask became {kept:o} after the child changed its own"))
    })
}

fn umask_threads() -> CaseResult {
    as_user(A, || {
        umask(0o022);
        let seen = with_thread(|| { let m = umask(0o011); umask(m); m }, || { umask(0o037); })?;
        want(seen == 0o037, format!("the other thread sees umask {seen:o}, expected 37"))?;
        let set = with_thread(|| umask(0o053), || {})?;
        want(set == 0o037, format!("the second thread found umask {set:o}, expected 37"))?;
        let now = umask(0o022);
        want(now == 0o053, format!("after the second thread's umask the first sees {now:o}, expected 53"))
    })
}

include!("suite_users_accounts.rs");
include!("suite_users_login.rs");
