//! Directories & links: independent POSIX assertions on the writable filesystem.
//! The shared runner forks each case and enforces its default 10-second deadline.
//! No missing operation is skipped, and no expected result depends on Breenix.
use libbreenix::error::Error;
use libbreenix::suite::{case, category, check, fail, suite, CaseResult, Suite};
use libbreenix::syscall::{nr, raw};
use libbreenix::types::Fd;
use libbreenix::{
    fs::{self, *},
    io, process, time,
};
use std::cell::RefCell;

const AT_FDCWD: u64 = (-100i64) as u64;
const UTIME_NOW: i64 = 1073741823;
const UTIME_OMIT: i64 = 1073741822;
#[cfg(target_arch = "x86_64")]
const ABI: [u64; 6] = [268, 260, 95, 105, 106, 280];
#[cfg(target_arch = "aarch64")]
const ABI: [u64; 6] = [53, 54, 166, 146, 144, 88];

fn request(n: u64, args: [u64; 4]) -> Result<u64, Error> {
    // SAFETY: callers retain NUL-terminated paths and ABI buffers through the call.
    let r = unsafe { raw::syscall4(n, args[0], args[1], args[2], args[3]) };
    Error::from_syscall(r as i64).map(|v| v as u64)
}
fn errno<T>(result: Result<T, Error>, expected: i32) -> CaseResult {
    errno_one_of(result, &[expected])
}
fn errno_one_of<T>(result: Result<T, Error>, expected: &[i32]) -> CaseResult {
    match result {
        Err(Error::Os(e))
            if expected
                .iter()
                .any(|n| e == libbreenix::Errno::from_raw(*n as i64)) =>
        {
            Ok(())
        }
        Err(e) => fail(format!("expected errno {expected:?}, got {e}")),
        Ok(_) => fail(format!("succeeded; expected errno {expected:?}")),
    }
}
fn cpath(p: &str) -> Vec<u8> {
    let mut b = p.as_bytes().to_vec();
    b.push(0);
    b
}
fn stat(p: &str, nofollow: bool) -> Result<Stat, Error> {
    let p = cpath(p);
    let mut s = Stat::new();
    request(
        nr::NEWFSTATAT,
        [
            AT_FDCWD,
            p.as_ptr() as u64,
            &mut s as *mut Stat as u64,
            if nofollow { 256 } else { 0 },
        ],
    )?;
    Ok(s)
}
fn chmod(p: &str, mode: u32) -> Result<(), Error> {
    let p = cpath(p);
    request(ABI[0], [AT_FDCWD, p.as_ptr() as u64, mode as u64, 0]).map(|_| ())
}
fn chown(p: &str, uid: u32, gid: u32) -> Result<(), Error> {
    let p = cpath(p);
    // fchownat has five arguments; both architectures use the Linux ABI.
    let r = unsafe {
        raw::syscall5(
            ABI[1],
            AT_FDCWD,
            p.as_ptr() as u64,
            uid as u64,
            gid as u64,
            0,
        )
    };
    Error::from_syscall(r as i64).map(|_| ())
}
fn umask(mode: u32) -> Result<u64, Error> {
    request(ABI[2], [mode as u64, 0, 0, 0])
}
fn unprivileged() -> CaseResult {
    request(ABI[4], [1001, 0, 0, 0])?;
    request(ABI[3], [1001, 0, 0, 0])?;
    Ok(())
}
struct Tree {
    root: String,
    paths: RefCell<Vec<String>>,
}
impl Tree {
    fn new() -> Result<Self, libbreenix::suite::CaseError> {
        umask(0)?;
        let root = format!("/tmp/directories-{}", process::getpid()?.raw());
        fs::mkdir(&root, 0o777)?;
        Ok(Self {
            root,
            paths: RefCell::new(Vec::new()),
        })
    }
    fn path(&self, name: &str) -> String {
        let p = format!("{}/{name}", self.root);
        self.paths.borrow_mut().push(p.clone());
        p
    }
    fn dir(&self, name: &str) -> Result<String, libbreenix::suite::CaseError> {
        let p = self.path(name);
        fs::mkdir(&p, 0o777)?;
        Ok(p)
    }
    fn file(&self, name: &str, data: &[u8]) -> Result<String, libbreenix::suite::CaseError> {
        let p = self.path(name);
        let fd = fs::open_with_mode(&p, O_CREAT | O_EXCL | O_RDWR, 0o666)?;
        let written = write_all(fd, data);
        let closed = io::close(fd);
        written?;
        closed?;
        Ok(p)
    }
    fn sym(&self, name: &str, target: &str) -> Result<String, libbreenix::suite::CaseError> {
        let p = self.path(name);
        fs::symlink(target, &p)?;
        Ok(p)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = process::chdir(b"/\0");
        // Best-effort cleanup never supplies evidence for an assertion. A broken
        // cleanup or killed case cannot affect another case's PID-specific tree.
        for p in self.paths.get_mut().iter().rev() {
            let _ = fs::unlink(p);
            let _ = fs::rmdir(p);
        }
        let _ = fs::rmdir(&self.root);
    }
}
fn write_all(fd: Fd, mut bytes: &[u8]) -> CaseResult {
    while !bytes.is_empty() {
        let n = io::write(fd, bytes)?;
        check(n > 0 && n <= bytes.len(), "invalid write count")?;
        bytes = &bytes[n..];
    }
    Ok(())
}
fn contents(fd: Fd, expected: &[u8]) -> CaseResult {
    fs::lseek(fd, 0, SEEK_SET)?;
    let mut data = vec![0; expected.len() + 1];
    let mut used = 0;
    while used < data.len() {
        let n = io::read(fd, &mut data[used..])?;
        check(n <= data.len() - used, "invalid read count")?;
        if n == 0 {
            break;
        }
        used += n;
    }
    check(&data[..used] == expected, "filesystem data differs")
}
fn bytes(p: &str, expected: &[u8]) -> CaseResult {
    let fd = File::open(p, O_RDONLY)?;
    contents(fd.fd(), expected)
}
fn cwd() -> Result<String, libbreenix::suite::CaseError> {
    let mut b = [0xa5; 256];
    let n = process::getcwd(&mut b)?;
    check(
        n > 0 && n <= b.len() && b[n - 1] == 0,
        "getcwd count or NUL invalid",
    )?;
    String::from_utf8(b[..n - 1].to_vec()).map_err(|e| e.to_string().into())
}
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    ino: u64,
}
/// Thin, unbuffered directory stream over getdents64 and lseek. This suite
/// measures kernel iteration/cookie semantics, not the separate libc interfaces
/// milestone. No entry is replayed from a userspace cache. Fixtures use short
/// names so a 32-byte syscall buffer admits exactly one Linux dirent record.
struct Directory {
    file: File,
}
impl Directory {
    fn open(p: &str) -> Result<Self, Error> {
        Ok(Self {
            file: File::open(p, O_RDONLY | O_DIRECTORY)?,
        })
    }
    fn readdir(&mut self) -> Result<Option<Entry>, libbreenix::suite::CaseError> {
        let mut b = [0u8; 32];
        let n = fs::getdents64(self.file.fd(), &mut b)?;
        if n == 0 {
            return Ok(None);
        }
        check(n >= 24 && n <= b.len(), "invalid dirent length")?;
        let len = u16::from_ne_bytes([b[16], b[17]]) as usize;
        check(len == n, "expected exactly one complete dirent")?;
        let end = b[19..n]
            .iter()
            .position(|v| *v == 0)
            .ok_or("dirent lacks NUL")?
            + 19;
        let name = String::from_utf8(b[19..end].to_vec()).map_err(|e| e.to_string())?;
        Ok(Some(Entry {
            name,
            ino: u64::from_ne_bytes(b[..8].try_into().unwrap()),
        }))
    }
    fn all(&mut self) -> Result<Vec<Entry>, libbreenix::suite::CaseError> {
        let mut entries = Vec::new();
        while let Some(e) = self.readdir()? {
            check(entries.len() < 32, "directory stream did not terminate")?;
            entries.push(e);
        }
        Ok(entries)
    }
    fn telldir(&self) -> Result<u64, Error> {
        fs::lseek(self.file.fd(), 0, SEEK_CUR)
    }
    fn seekdir(&mut self, cookie: u64) -> CaseResult {
        check(
            fs::lseek(self.file.fd(), cookie as i64, SEEK_SET)? == cookie,
            "directory seek returned a different cookie",
        )
    }
    fn rewinddir(&mut self) -> CaseResult {
        self.seekdir(0)
    }
}
fn listing() -> Result<Tree, libbreenix::suite::CaseError> {
    let f = Tree::new()?;
    f.file("beta", b"b")?;
    f.file("alpha", b"a")?;
    f.dir("dir")?;
    Ok(f)
}
fn atomic_replace() -> CaseResult {
    let f = Tree::new()?;
    let src = f.file("src", b"NEW-DATA")?;
    let dst = f.file("dst", b"OLD-DATA")?;
    let (ready_r, ready_w) = io::pipe()?;
    let (done_r, done_w) = io::pipe()?;
    match process::fork()? {
        process::ForkResult::Child => {
            let result = (|| -> CaseResult {
                io::close(ready_r)?;
                io::close(done_w)?;
                bytes(&dst, b"OLD-DATA")?;
                write_all(ready_w, b"r")?;
                io::fcntl(done_r, 4, O_NONBLOCK as i64)?;
                loop {
                    let fd = File::open(&dst, O_RDONLY)?;
                    let mut b = [0; 9];
                    let n = io::read(fd.fd(), &mut b)?;
                    check(
                        n == 8 && (&b[..8] == b"OLD-DATA" || &b[..8] == b"NEW-DATA"),
                        "reader saw missing or partial replacement",
                    )?;
                    match io::read(done_r, &mut [0; 1]) {
                        Ok(1) => break,
                        Err(Error::Os(libbreenix::Errno::EAGAIN)) => {}
                        other => return fail(format!("completion pipe: {other:?}")),
                    }
                }
                bytes(&dst, b"NEW-DATA")
            })();
            process::exit(if result.is_ok() { 0 } else { 1 });
        }
        process::ForkResult::Parent(pid) => {
            io::close(ready_w)?;
            io::close(done_r)?;
            check(io::read(ready_r, &mut [0; 1])? == 1, "reader did not start")?;
            let renamed = fs::rename(&src, &dst);
            write_all(done_w, b"d")?;
            io::close(done_w)?;
            io::close(ready_r)?;
            let mut status = 0;
            process::waitpid(pid.raw() as i32, &mut status, 0)?;
            renamed?;
            check(
                process::wifexited(status) && process::wexitstatus(status) == 0,
                "concurrent reader observed a missing or partial replacement",
            )?;
            bytes(&dst, b"NEW-DATA")
        }
    }
}
#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}
fn set_times(p: &str, values: [(i64, i64); 2], flags: u64) -> Result<(), Error> {
    let p = cpath(p);
    let times = values.map(|(sec, nsec)| Timespec { sec, nsec });
    request(
        ABI[5],
        [AT_FDCWD, p.as_ptr() as u64, times.as_ptr() as u64, flags],
    )
    .map(|_| ())
}
fn old_times(p: &str) -> Result<(), Error> {
    set_times(p, [(1000000000, 0), (1000000001, 0)], 0)
}
fn atime(s: &Stat) -> (i64, i64) {
    (s.st_atime, s.st_atime_nsec)
}
fn mtime(s: &Stat) -> (i64, i64) {
    (s.st_mtime, s.st_mtime_nsec)
}
fn ctime(s: &Stat) -> (i64, i64) {
    (s.st_ctime, s.st_ctime_nsec)
}
fn realtime() -> Result<(i64, i64), Error> {
    let mut t = libbreenix::Timespec::new();
    time::clock_gettime(time::CLOCK_REALTIME, &mut t)?;
    Ok((t.tv_sec, t.tv_nsec))
}
fn in_window(t: (i64, i64), before: (i64, i64), after: (i64, i64)) -> bool {
    // POSIX permits rounding down to the filesystem's timestamp resolution.
    // ext2 stores whole seconds; allow that floor, never a stale epoch value.
    t >= (before.0, 0) && t <= after && (0..1000000000).contains(&t.1)
}

static SUITE: Suite = suite("directories", "Directories & links", &[
    category("mkdir-rmdir", "mkdir & rmdir", &[
        case("create", "mkdir creates a directory with the requested mode", mkdir_rmdir_create),
        case("existing-dir", "mkdir on an existing directory fails with EEXIST", mkdir_rmdir_existing_dir),
        case("existing-file", "mkdir on an existing file fails with EEXIST", mkdir_rmdir_existing_file),
        case("missing-parent", "mkdir with a missing parent fails with ENOENT", mkdir_rmdir_missing_parent),
        case("file-parent", "mkdir beneath a regular file fails with ENOTDIR", mkdir_rmdir_file_parent),
        case("empty", "rmdir removes an empty directory", mkdir_rmdir_empty),
        case("nonempty-file", "rmdir with a file child fails with ENOTEMPTY and preserves the tree", mkdir_rmdir_nonempty_file),
        case("nonempty-dir", "rmdir with a directory child fails with ENOTEMPTY", mkdir_rmdir_nonempty_dir),
        case("regular-file", "rmdir on a regular file fails with ENOTDIR", mkdir_rmdir_regular_file),
        case("missing", "rmdir on a missing path fails with ENOENT", mkdir_rmdir_missing),
        case("symlink", "rmdir does not follow a final symlink to a directory", mkdir_rmdir_symlink),
        case("parent-nlink", "mkdir and rmdir update the parent directory link count", mkdir_rmdir_parent_nlink),
    ]),
    category("readdir", "readdir & seekdir", &[
        case("entries", "readdir returns every child exactly once without imposing lexical order", readdir_entries),
        case("dot", "readdir includes dot with the directory inode", readdir_dot),
        case("dotdot", "readdir includes dot-dot with the parent inode", readdir_dotdot),
        case("inodes", "readdir child inode numbers agree with fresh pathname stat", readdir_inodes),
        case("eof", "readdir at end stays at end", readdir_eof),
        case("rewind", "rewinddir replays an unchanged directory in its original order", readdir_rewind),
        case("tell-seek", "seekdir to a telldir cookie replays the remaining entries in order", readdir_tell_seek),
        case("tell-after-rewind", "telldir and seekdir work with a fresh cookie after rewinddir", readdir_tell_after_rewind),
        case("two-streams", "independent directory streams have independent positions", readdir_two_streams),
        case("not-directory", "opening a regular file as a directory fails with ENOTDIR", readdir_not_directory),
    ]),
    category("links", "hard links & unlink", &[
        case("identity", "hard links share an inode and report two links", links_identity),
        case("shared-write", "writes through a hard link are visible by reopening the original", links_shared_write),
        case("unlink-one", "unlink of one hard link preserves data and decrements nlink", links_unlink_one),
        case("open-unlink", "an open descriptor sees link counts two, one and zero and retains data", links_open_unlink),
        case("destination-exists", "link to an existing destination fails with EEXIST", links_destination_exists),
        case("source-missing", "link of a missing source fails with ENOENT", links_source_missing),
        case("directory", "hard linking a directory fails with EPERM", links_directory),
        case("cross-directory", "hard links across directories share data and link counts", links_cross_directory),
        case("unlink-missing", "unlink of a missing name fails with ENOENT", links_unlink_missing),
    ]),
    category("symlinks", "symlinks & readlink", &[
        case("absolute", "an absolute symlink resolves to its target", symlinks_absolute),
        case("relative", "a relative symlink resolves from its containing directory", symlinks_relative),
        case("dangling", "a dangling symlink exists in lstat but opening it fails with ENOENT", symlinks_dangling),
        case("existing", "symlink at an existing name fails with EEXIST", symlinks_existing),
        case("readlink", "readlink returns target bytes without a terminating NUL", symlinks_readlink),
        case("truncate", "readlink silently truncates to the buffer size without a NUL", symlinks_truncate),
        case("readlink-regular", "readlink on a regular file fails with EINVAL", symlinks_readlink_regular),
        case("self-loop", "opening a self-referential symlink fails with ELOOP", symlinks_self_loop),
        case("mutual-loop", "opening a two-link cycle fails with ELOOP", symlinks_mutual_loop),
        case("nofollow", "O_NOFOLLOW rejects a final symlink with ELOOP", symlinks_nofollow),
        case("nofollow-prefix", "O_NOFOLLOW still follows symlinks in prefix components", symlinks_nofollow_prefix),
        case("unlink-link", "unlink removes the symlink rather than its target", symlinks_unlink_link),
    ]),
    category("rename", "rename & atomic replace", &[
        case("file", "rename moves a file while preserving inode and contents", rename_file),
        case("replace-open", "rename replaces a file while old open descriptors retain the replaced inode", rename_replace_open),
        case("atomic-replace", "a concurrent reader sees the destination continuously as whole old or new data", rename_atomic_replace),
        case("empty-dir", "rename of a directory onto an empty directory replaces it", rename_empty_dir),
        case("nonempty-dir", "rename onto a non-empty directory fails with EEXIST or ENOTEMPTY", rename_nonempty_dir),
        case("cross-directory", "rename moves a file across directories", rename_cross_directory),
        case("directory-parent", "moving a directory updates dot-dot and both parent link counts", rename_directory_parent),
        case("same-name", "rename of a name onto itself succeeds without altering data", rename_same_name),
        case("same-inode", "rename between hard links to the same inode preserves both names", rename_same_inode),
        case("file-to-dir", "rename of a file onto a directory fails with EISDIR", rename_file_to_dir),
        case("dir-to-file", "rename of a directory onto a file fails with ENOTDIR", rename_dir_to_file),
        case("descendant", "rename of a directory into its descendant fails with EINVAL", rename_descendant),
        case("symlink", "rename moves a symlink itself and preserves its target text", rename_symlink),
        case("missing-source", "rename of a missing source fails with ENOENT and preserves the destination", rename_missing_source),
    ]),
    category("cwd", "getcwd, chdir & relative paths", &[
        case("absolute", "chdir followed by getcwd reports the absolute directory path", cwd_absolute),
        case("relative", "relative opens after chdir resolve from the working directory", cwd_relative),
        case("parent", "chdir dot-dot moves to the parent directory", cwd_parent),
        case("relative-create", "mkdir and rename with relative paths operate in the working directory", cwd_relative_create),
        case("renamed", "getcwd follows a renamed working directory to its new pathname", cwd_renamed),
        case("ancestor-renamed", "getcwd follows a renamed ancestor of the working directory", cwd_ancestor_renamed),
        case("removed", "getcwd of a removed working directory fails with ENOENT", cwd_removed),
        case("small-buffer", "getcwd with an insufficient buffer fails with ERANGE", cwd_small_buffer),
        case("not-dir", "chdir to a regular file fails with ENOTDIR and keeps cwd", cwd_not_dir),
        case("missing", "chdir to a missing path fails with ENOENT and keeps cwd", cwd_missing),
    ]),
    category("permissions", "chmod, chown, access & umask", &[
        case("chmod-file", "chmod changes file permission bits reported by fresh stat", permissions_chmod_file),
        case("chmod-dir", "chmod changes directory permission bits", permissions_chmod_dir),
        case("chmod-missing", "chmod of a missing path fails with ENOENT", permissions_chmod_missing),
        case("chown", "chown updates the UID and GID reported by fresh stat", permissions_chown),
        case("chown-unchanged", "chown with minus one leaves the corresponding owner unchanged", permissions_chown_unchanged),
        case("umask-return", "umask returns the previous mask and restricts it to permission bits", permissions_umask_return),
        case("umask-file", "umask applies at regular-file creation", permissions_umask_file),
        case("umask-dir", "umask applies at directory creation", permissions_umask_dir),
        case("chmod-umask", "umask does not mask chmod permission bits", permissions_chmod_umask),
        case("access-exists", "access F_OK reports existence even without permission bits", permissions_access_exists),
        case("access-owner", "access checks owner permission bits using real credentials", permissions_access_owner),
        case("access-group", "access checks group permission bits using real credentials", permissions_access_group),
        case("access-other", "access checks other permission bits using real credentials", permissions_access_other),
        case("access-execute", "access checks execute permission bits using real credentials", permissions_access_execute),
        case("search-denied", "path traversal without directory search permission fails with EACCES", permissions_search_denied),
        case("root-execute", "root access X_OK on a regular file with no execute bits fails with EACCES", permissions_root_execute),
    ]),
    category("timestamps", "utimensat & time updates", &[
        case("explicit", "utimensat sets explicit access and modification times", timestamps_explicit),
        case("now", "UTIME_NOW sets both times to the current realtime clock", timestamps_now),
        case("omit-atime", "UTIME_OMIT preserves atime while setting mtime", timestamps_omit_atime),
        case("omit-mtime", "UTIME_OMIT preserves mtime while setting atime", timestamps_omit_mtime),
        case("omit-both", "two UTIME_OMIT values preserve atime mtime and ctime", timestamps_omit_both),
        case("now-omit", "UTIME_NOW and UTIME_OMIT can be used in the same request", timestamps_now_omit),
        case("invalid-nsec", "utimensat rejects an ordinary nanosecond value of one billion with EINVAL", timestamps_invalid_nsec),
        case("missing", "utimensat of a missing file fails with ENOENT", timestamps_missing),
        case("symlink-follow", "utimensat follows symlinks by default", timestamps_symlink_follow),
        case("symlink-nofollow", "utimensat AT_SYMLINK_NOFOLLOW updates only the symlink", timestamps_symlink_nofollow),
        case("write-update", "writing file data updates mtime and ctime", timestamps_write_update),
        case("parent-create", "creating a child updates its parent directory mtime and ctime", timestamps_parent_create),
        case("parent-unlink", "unlink updates the parent directory mtime and ctime", timestamps_parent_unlink),
        case("parent-rename", "cross-directory rename updates both parent directories timestamps", timestamps_parent_rename),
    ]),
]);
fn main() {
    SUITE.run()
}

fn mkdir_rmdir_create() -> CaseResult {
    let f = Tree::new()?;
    let p = f.path("dir");
    fs::mkdir(&p, 0o751)?;
    let s = stat(&p, false)?;
    check(
        s.is_dir() && s.st_mode & 0o777 == 0o751,
        "directory type or mode differs",
    )
}

fn mkdir_rmdir_existing_dir() -> CaseResult {
    let f = Tree::new()?;
    errno(fs::mkdir(&f.root, 0o777), 17)
}

fn mkdir_rmdir_existing_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"old")?;
    errno(fs::mkdir(&p, 0o777), 17)?;
    bytes(&p, b"old")
}

fn mkdir_rmdir_missing_parent() -> CaseResult {
    let f = Tree::new()?;
    errno(fs::mkdir(&f.path("absent/dir"), 0o777), 2)
}

fn mkdir_rmdir_file_parent() -> CaseResult {
    let f = Tree::new()?;
    f.file("file", b"x")?;
    errno(fs::mkdir(&f.path("file/dir"), 0o777), 20)
}

fn mkdir_rmdir_empty() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    fs::rmdir(&p)?;
    errno(stat(&p, false), 2)
}

fn mkdir_rmdir_nonempty_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    let child = f.file("dir/file", b"stay")?;
    errno(fs::rmdir(&p), 39)?;
    bytes(&child, b"stay")
}

fn mkdir_rmdir_nonempty_dir() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    let c = f.dir("dir/child")?;
    errno(fs::rmdir(&p), 39)?;
    check(stat(&c, false)?.is_dir(), "child disappeared")
}

fn mkdir_rmdir_regular_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"stay")?;
    errno(fs::rmdir(&p), 20)?;
    bytes(&p, b"stay")
}

fn mkdir_rmdir_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(fs::rmdir(&f.path("absent")), 2)
}

fn mkdir_rmdir_symlink() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    let p = f.sym("sym", &d)?;
    errno(fs::rmdir(&p), 20)?;
    check(
        stat(&d, false)?.is_dir() && stat(&p, true)?.is_symlink(),
        "rmdir damaged link or target",
    )
}

fn mkdir_rmdir_parent_nlink() -> CaseResult {
    let f = Tree::new()?;
    let before = stat(&f.root, false)?.st_nlink;
    let p = f.dir("dir")?;
    check(
        stat(&f.root, false)?.st_nlink == before + 1,
        "mkdir did not increment parent nlink",
    )?;
    fs::rmdir(&p)?;
    check(
        stat(&f.root, false)?.st_nlink == before,
        "rmdir did not restore parent nlink",
    )
}

fn readdir_entries() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let entries = d.all()?;
    let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    check(
        names == [".", "..", "alpha", "beta", "dir"],
        "entries missing or duplicated",
    )
}

fn readdir_dot() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let entries = d.all()?;
    let ino = stat(&f.root, false)?.st_ino;
    check(
        entries
            .iter()
            .filter(|e| e.name == "." && e.ino == ino)
            .count()
            == 1,
        "dot missing or wrong inode",
    )
}

fn readdir_dotdot() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let entries = d.all()?;
    let ino = stat("/tmp", false)?.st_ino;
    check(
        entries
            .iter()
            .filter(|e| e.name == ".." && e.ino == ino)
            .count()
            == 1,
        "dot-dot missing or wrong inode",
    )
}

fn readdir_inodes() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    for e in d.all()? {
        check(
            e.ino == stat(&format!("{}/{}", f.root, e.name), false)?.st_ino,
            "dirent inode differs from stat",
        )?;
    }
    Ok(())
}

fn readdir_eof() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    d.all()?;
    check(
        d.readdir()?.is_none() && d.readdir()?.is_none(),
        "EOF produced another entry",
    )
}

fn readdir_rewind() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let before = d.all()?;
    d.rewinddir()?;
    check(d.all()? == before, "rewind changed entry sequence")
}

fn readdir_tell_seek() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    check(d.readdir()?.is_some(), "empty stream")?;
    let cookie = d.telldir()?;
    let tail = d.all()?;
    check(!tail.is_empty(), "missing tail")?;
    d.seekdir(cookie)?;
    check(d.all()? == tail, "seek did not restore the saved position")
}

fn readdir_tell_after_rewind() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    d.all()?;
    d.rewinddir()?;
    check(d.readdir()?.is_some(), "rewind empty")?;
    let cookie = d.telldir()?;
    let tail = d.all()?;
    d.seekdir(cookie)?;
    check(d.all()? == tail, "fresh post-rewind cookie failed")
}

fn readdir_two_streams() -> CaseResult {
    let f = listing()?;
    let mut a = Directory::open(&f.root)?;
    let mut b = Directory::open(&f.root)?;
    let first = a.readdir()?;
    a.all()?;
    check(b.readdir()? == first, "one stream moved the other")
}

fn readdir_not_directory() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    errno(Directory::open(&p), 20)
}

fn links_identity() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"one")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    let x = stat(&a, false)?;
    let y = stat(&b, false)?;
    check(
        x.st_dev == y.st_dev && x.st_ino == y.st_ino && x.st_nlink == 2 && y.st_nlink == 2,
        "hard-link identity or nlink differs",
    )
}

fn links_shared_write() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"one")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    let fd = File::open(&b, O_WRONLY)?;
    write_all(fd.fd(), b"two")?;
    drop(fd);
    bytes(&a, b"two")
}

fn links_unlink_one() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    fs::unlink(&a)?;
    errno(stat(&a, false), 2)?;
    check(stat(&b, false)?.st_nlink == 1, "survivor nlink is not one")?;
    bytes(&b, b"stay")
}

fn links_open_unlink() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let fd = File::open(&a, O_RDWR)?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    check(fs::fstat(fd.fd())?.st_nlink == 2, "open nlink not two")?;
    fs::unlink(&a)?;
    check(fs::fstat(fd.fd())?.st_nlink == 1, "open nlink not one")?;
    fs::unlink(&b)?;
    check(fs::fstat(fd.fd())?.st_nlink == 0, "open nlink not zero")?;
    errno(stat(&b, false), 2)?;
    contents(fd.fd(), b"stay")?;
    fs::lseek(fd.fd(), 0, SEEK_SET)?;
    write_all(fd.fd(), b"live")?;
    contents(fd.fd(), b"live")
}

fn links_destination_exists() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"a")?;
    let b = f.file("b", b"b")?;
    errno(fs::link(&a, &b), 17)?;
    bytes(&b, b"b")
}

fn links_source_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(fs::link(&f.path("absent"), &f.path("b")), 2)
}

fn links_directory() -> CaseResult {
    let f = Tree::new()?;
    errno(fs::link(&f.root, &f.path("b")), 1)
}

fn links_cross_directory() -> CaseResult {
    let f = Tree::new()?;
    f.dir("dir")?;
    let a = f.file("a", b"data")?;
    let b = f.path("dir/b");
    fs::link(&a, &b)?;
    check(
        stat(&a, false)?.st_ino == stat(&b, false)?.st_ino && stat(&b, false)?.st_nlink == 2,
        "cross-directory link differs",
    )?;
    bytes(&b, b"data")
}

fn links_unlink_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(fs::unlink(&f.path("absent")), 2)
}

fn symlinks_absolute() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"data")?;
    let s = f.sym("sym", &a)?;
    bytes(&s, b"data")
}

fn symlinks_relative() -> CaseResult {
    let f = Tree::new()?;
    f.dir("dir")?;
    f.file("a", b"data")?;
    let s = f.sym("dir/sym", "../a")?;
    process::chdir(b"/\0")?;
    bytes(&s, b"data")
}

fn symlinks_dangling() -> CaseResult {
    let f = Tree::new()?;
    let s = f.sym("sym", "absent")?;
    check(
        stat(&s, true)?.is_symlink(),
        "lstat did not report a symlink",
    )?;
    errno(fs::open(&s, O_RDONLY), 2)
}

fn symlinks_existing() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("a", b"stay")?;
    errno(fs::symlink("absent", &p), 17)?;
    bytes(&p, b"stay")
}

fn symlinks_readlink() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("sym", "target")?;
    let mut b = [0xa5; 16];
    let n = fs::readlink(&p, &mut b)?;
    check(
        n == 6 && &b[..6] == b"target" && b[6..].iter().all(|v| *v == 0xa5),
        "readlink bytes, count or terminator differ",
    )
}

fn symlinks_truncate() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("sym", "long-target")?;
    let mut b = [0xa5; 5];
    let n = fs::readlink(&p, &mut b[..4])?;
    check(
        n == 4 && &b[..4] == b"long" && b[4] == 0xa5,
        "readlink truncation differs",
    )
}

fn symlinks_readlink_regular() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("a", b"x")?;
    errno(fs::readlink(&p, &mut [0; 16]), 22)
}

fn symlinks_self_loop() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("sym", "sym")?;
    errno(fs::open(&p, O_RDONLY), 40)
}

fn symlinks_mutual_loop() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("a", "b")?;
    f.sym("b", "a")?;
    errno(fs::open(&p, O_RDONLY), 40)
}

fn symlinks_nofollow() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"x")?;
    let s = f.sym("sym", &a)?;
    errno(fs::open(&s, O_RDONLY | 0x20000), 40)
}

fn symlinks_nofollow_prefix() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    f.file("dir/a", b"data")?;
    f.sym("sym", &d)?;
    let fd = File::open(&f.path("sym/a"), O_RDONLY | 0x20000)?;
    contents(fd.fd(), b"data")
}

fn symlinks_unlink_link() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let p = f.sym("sym", &a)?;
    fs::unlink(&p)?;
    errno(stat(&p, true), 2)?;
    bytes(&a, b"stay")
}

fn rename_file() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"data")?;
    let ino = stat(&a, false)?.st_ino;
    let b = f.path("b");
    fs::rename(&a, &b)?;
    errno(stat(&a, false), 2)?;
    check(stat(&b, false)?.st_ino == ino, "rename changed inode")?;
    bytes(&b, b"data")
}

fn rename_replace_open() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"new")?;
    let b = f.file("b", b"old")?;
    let old = File::open(&b, O_RDONLY)?;
    let new = File::open(&a, O_RDONLY)?;
    fs::rename(&a, &b)?;
    errno(stat(&a, false), 2)?;
    check(
        stat(&b, false)?.st_ino == fs::fstat(new.fd())?.st_ino
            && fs::fstat(old.fd())?.st_nlink == 0,
        "replacement inode or old nlink differs",
    )?;
    bytes(&b, b"new")?;
    contents(old.fd(), b"old")
}

fn rename_atomic_replace() -> CaseResult {
    atomic_replace()
}

fn rename_empty_dir() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let ino = stat(&a, false)?.st_ino;
    f.file("a/file", b"stay")?;
    let b = f.dir("b")?;
    let out = f.path("b/file");
    fs::rename(&a, &b)?;
    errno(stat(&a, false), 2)?;
    check(
        stat(&b, false)?.st_ino == ino,
        "directory replacement changed inode",
    )?;
    bytes(&out, b"stay")
}

fn rename_nonempty_dir() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.dir("b")?;
    let c = f.file("b/file", b"stay")?;
    errno_one_of(fs::rename(&a, &b), &[17, 39])?;
    check(stat(&a, false)?.is_dir(), "source disappeared")?;
    bytes(&c, b"stay")
}

fn rename_cross_directory() -> CaseResult {
    let f = Tree::new()?;
    f.dir("a")?;
    f.dir("b")?;
    let src = f.file("a/file", b"data")?;
    let dst = f.path("b/file");
    fs::rename(&src, &dst)?;
    errno(stat(&src, false), 2)?;
    bytes(&dst, b"data")
}

fn rename_directory_parent() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.dir("b")?;
    let src = f.dir("a/dir")?;
    let dst = f.path("b/dir");
    let an = stat(&a, false)?.st_nlink;
    let bn = stat(&b, false)?.st_nlink;
    fs::rename(&src, &dst)?;
    check(
        stat(&format!("{dst}/.."), false)?.st_ino == stat(&b, false)?.st_ino,
        "dot-dot still names old parent",
    )?;
    check(
        stat(&a, false)?.st_nlink == an - 1 && stat(&b, false)?.st_nlink == bn + 1,
        "parent nlink not transferred",
    )
}

fn rename_same_name() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    fs::rename(&a, &a)?;
    bytes(&a, b"stay")
}

fn rename_same_inode() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    fs::rename(&a, &b)?;
    check(
        stat(&a, false)?.st_nlink == 2 && stat(&a, false)?.st_ino == stat(&b, false)?.st_ino,
        "same-inode rename removed a name",
    )
}

fn rename_file_to_dir() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let b = f.dir("b")?;
    errno(fs::rename(&a, &b), 21)?;
    bytes(&a, b"stay")
}

fn rename_dir_to_file() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.file("b", b"stay")?;
    errno(fs::rename(&a, &b), 20)?;
    bytes(&b, b"stay")
}

fn rename_descendant() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    f.dir("a/dir")?;
    errno(fs::rename(&a, &f.path("a/dir/moved")), 22)
}

fn rename_symlink() -> CaseResult {
    let f = Tree::new()?;
    let a = f.sym("a", "absent")?;
    let b = f.path("b");
    fs::rename(&a, &b)?;
    errno(stat(&a, true), 2)?;
    let mut buf = [0; 16];
    let n = fs::readlink(&b, &mut buf)?;
    check(
        stat(&b, true)?.is_symlink() && &buf[..n] == b"absent",
        "rename followed or changed symlink",
    )
}

fn rename_missing_source() -> CaseResult {
    let f = Tree::new()?;
    let b = f.file("b", b"stay")?;
    errno(fs::rename(&f.path("absent"), &b), 2)?;
    bytes(&b, b"stay")
}

fn cwd_absolute() -> CaseResult {
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    check(cwd()? == f.root, "getcwd differs from chdir target")
}

fn cwd_relative() -> CaseResult {
    let f = Tree::new()?;
    f.dir("dir")?;
    f.file("dir/file", b"data")?;
    process::chdir(&cpath(&f.root))?;
    process::chdir(b"dir\0")?;
    bytes("file", b"data")
}

fn cwd_parent() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    process::chdir(&cpath(&p))?;
    process::chdir(b"..\0")?;
    check(cwd()? == f.root, "dot-dot cwd differs")
}

fn cwd_relative_create() -> CaseResult {
    let f = Tree::new()?;
    let a = f.path("a");
    let b = f.path("b");
    process::chdir(&cpath(&f.root))?;
    fs::mkdir("a", 0o755)?;
    fs::rename("a", "b")?;
    errno(stat(&a, false), 2)?;
    check(
        stat(&b, false)?.is_dir(),
        "relative rename destination missing",
    )
}

fn cwd_renamed() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.path("b");
    process::chdir(&cpath(&a))?;
    fs::rename(&a, &b)?;
    check(cwd()? == b, "getcwd retained old name")
}

fn cwd_ancestor_renamed() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let d = f.dir("a/dir")?;
    let b = f.path("b");
    f.path("b/dir");
    process::chdir(&cpath(&d))?;
    fs::rename(&a, &b)?;
    check(cwd()? == format!("{b}/dir"), "getcwd retained old ancestor")
}

fn cwd_removed() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    process::chdir(&cpath(&d))?;
    fs::rmdir(&d)?;
    errno(process::getcwd(&mut [0; 256]), 2)
}

fn cwd_small_buffer() -> CaseResult {
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    errno(process::getcwd(&mut [0; 2]), 34)
}

fn cwd_not_dir() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    process::chdir(&cpath(&f.root))?;
    errno(process::chdir(&cpath(&p)), 20)?;
    check(cwd()? == f.root, "failed chdir changed cwd")
}

fn cwd_missing() -> CaseResult {
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    errno(process::chdir(&cpath(&f.path("absent"))), 2)?;
    check(cwd()? == f.root, "failed chdir changed cwd")
}

fn permissions_chmod_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chmod(&p, 0o654)?;
    check(
        stat(&p, false)?.st_mode & 0o7777 == 0o654,
        "chmod mode differs",
    )
}

fn permissions_chmod_dir() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    chmod(&p, 0o711)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o711,
        "directory chmod mode differs",
    )
}

fn permissions_chmod_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(chmod(&f.path("absent"), 0o600), 2)
}

fn permissions_chown() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 1234, 2345)?;
    let s = stat(&p, false)?;
    check(
        s.st_uid == 1234 && s.st_gid == 2345,
        "chown ownership differs",
    )
}

fn permissions_chown_unchanged() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 1234, 2345)?;
    chown(&p, u32::MAX, 3456)?;
    let s = stat(&p, false)?;
    check(
        s.st_uid == 1234 && s.st_gid == 3456,
        "minus-one UID was not preserved",
    )?;
    chown(&p, 4567, u32::MAX)?;
    let s = stat(&p, false)?;
    check(
        s.st_uid == 4567 && s.st_gid == 3456,
        "minus-one GID was not preserved",
    )
}

fn permissions_umask_return() -> CaseResult {
    umask(0o027)?;
    check(
        umask(0o7022)? == 0o027 && umask(0)? == 0o022,
        "umask old value or mask bits differ",
    )
}

fn permissions_umask_file() -> CaseResult {
    let f = Tree::new()?;
    umask(0o027)?;
    let p = f.path("file");
    let fd = fs::open_with_mode(&p, O_CREAT | O_EXCL | O_RDWR, 0o666)?;
    io::close(fd)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o640,
        "file creation ignored umask",
    )
}

fn permissions_umask_dir() -> CaseResult {
    let f = Tree::new()?;
    umask(0o027)?;
    let p = f.path("dir");
    fs::mkdir(&p, 0o777)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o750,
        "mkdir ignored umask",
    )
}

fn permissions_chmod_umask() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    umask(0o777)?;
    chmod(&p, 0o765)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o765,
        "chmod applied umask",
    )
}

fn permissions_access_exists() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chmod(&p, 0)?;
    unprivileged()?;
    fs::access(&p, F_OK)?;
    errno(fs::access(&f.path("absent"), F_OK), 2)
}

fn permissions_access_owner() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 1001, 1001)?;
    chmod(&p, 0o400)?;
    unprivileged()?;
    fs::access(&p, R_OK)?;
    errno(fs::access(&p, W_OK | X_OK), 13)
}

fn permissions_access_group() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 2001, 1001)?;
    chmod(&p, 0o40)?;
    unprivileged()?;
    fs::access(&p, R_OK)?;
    errno(fs::access(&p, W_OK | X_OK), 13)
}

fn permissions_access_other() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 2001, 2001)?;
    chmod(&p, 0o4)?;
    unprivileged()?;
    fs::access(&p, R_OK)?;
    errno(fs::access(&p, W_OK | X_OK), 13)
}

fn permissions_access_execute() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 1001, 1001)?;
    chmod(&p, 0o100)?;
    unprivileged()?;
    fs::access(&p, X_OK)?;
    errno(fs::access(&p, R_OK | W_OK), 13)
}

fn permissions_search_denied() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    let p = f.file("dir/file", b"x")?;
    chmod(&d, 0o666)?;
    unprivileged()?;
    errno(fs::access(&p, F_OK), 13)
}

fn permissions_root_execute() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chmod(&p, 0o600)?;
    errno(fs::access(&p, X_OK), 13)
}

fn timestamps_explicit() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    set_times(&p, [(1000000000, 0), (1000000001, 0)], 0)?;
    let s = stat(&p, false)?;
    check(
        atime(&s) == (1000000000, 0) && mtime(&s) == (1000000001, 0),
        "explicit times differ",
    )
}

fn timestamps_now() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let before = realtime()?;
    set_times(&p, [(0, UTIME_NOW), (0, UTIME_NOW)], 0)?;
    let after = realtime()?;
    let s = stat(&p, false)?;
    check(
        in_window(atime(&s), before, after) && in_window(mtime(&s), before, after),
        "UTIME_NOW outside clock window",
    )
}

fn timestamps_omit_atime() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    set_times(&p, [(0, UTIME_OMIT), (1000000002, 0)], 0)?;
    let s = stat(&p, false)?;
    check(
        atime(&s) == atime(&old) && mtime(&s) == (1000000002, 0),
        "OMIT atime or explicit mtime differs",
    )
}

fn timestamps_omit_mtime() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    set_times(&p, [(1000000002, 0), (0, UTIME_OMIT)], 0)?;
    let s = stat(&p, false)?;
    check(
        mtime(&s) == mtime(&old) && atime(&s) == (1000000002, 0),
        "OMIT mtime or explicit atime differs",
    )
}

fn timestamps_omit_both() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    set_times(&p, [(0, UTIME_OMIT), (0, UTIME_OMIT)], 0)?;
    let s = stat(&p, false)?;
    check(
        atime(&s) == atime(&old) && mtime(&s) == mtime(&old) && ctime(&s) == ctime(&old),
        "two OMIT values changed timestamps",
    )
}

fn timestamps_now_omit() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    let before = realtime()?;
    set_times(&p, [(0, UTIME_NOW), (0, UTIME_OMIT)], 0)?;
    let after = realtime()?;
    let s = stat(&p, false)?;
    check(
        in_window(atime(&s), before, after) && mtime(&s) == mtime(&old),
        "mixed NOW and OMIT differ",
    )
}

fn timestamps_invalid_nsec() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    errno(set_times(&p, [(0, 1000000000), (0, 0)], 0), 22)
}

fn timestamps_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(set_times(&f.path("absent"), [(1, 0), (2, 0)], 0), 2)
}

fn timestamps_symlink_follow() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    let s = f.sym("sym", &p)?;
    set_times(&s, [(1000000000, 0), (1000000001, 0)], 0)?;
    check(
        mtime(&stat(&p, false)?) == (1000000001, 0),
        "utimensat did not update target",
    )
}

fn timestamps_symlink_nofollow() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    let s = f.sym("sym", &p)?;
    set_times(&s, [(1000000003, 0), (1000000004, 0)], 256)?;
    check(
        mtime(&stat(&s, true)?) == (1000000004, 0) && mtime(&stat(&p, false)?) == mtime(&old),
        "nofollow changed target or missed link",
    )
}

fn timestamps_write_update() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let fd = File::open(&p, O_WRONLY)?;
    let before = realtime()?;
    write_all(fd.fd(), b"y")?;
    drop(fd);
    let after = realtime()?;
    let s = stat(&p, false)?;
    check(
        in_window(mtime(&s), before, after) && in_window(ctime(&s), before, after),
        "write timestamps not current",
    )
}

fn timestamps_parent_create() -> CaseResult {
    let f = Tree::new()?;
    old_times(&f.root)?;
    let before = realtime()?;
    f.file("file", b"x")?;
    let after = realtime()?;
    let s = stat(&f.root, false)?;
    check(
        in_window(mtime(&s), before, after) && in_window(ctime(&s), before, after),
        "create parent times not current",
    )
}

fn timestamps_parent_unlink() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&f.root)?;
    let before = realtime()?;
    fs::unlink(&p)?;
    let after = realtime()?;
    let s = stat(&f.root, false)?;
    check(
        in_window(mtime(&s), before, after) && in_window(ctime(&s), before, after),
        "unlink parent times not current",
    )
}

fn timestamps_parent_rename() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.dir("b")?;
    let src = f.file("a/file", b"x")?;
    let dst = f.path("b/file");
    old_times(&a)?;
    old_times(&b)?;
    let before = realtime()?;
    fs::rename(&src, &dst)?;
    let after = realtime()?;
    for p in [&a, &b] {
        let s = stat(p, false)?;
        check(
            in_window(mtime(&s), before, after) && in_window(ctime(&s), before, after),
            "rename parent times not current",
        )?;
    }
    Ok(())
}
