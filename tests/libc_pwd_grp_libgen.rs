//! Host harness for the C library's user and group databases and pathname functions
//! (libs/libbreenix-libc/src/{pwd,grp,libgen}.rs), whose unit tests cannot run in
//! their own crate: libbreenix-libc defines a panic handler and the C entry points.
//!
//! The modules are compiled here unchanged. The crate functions they call (open,
//! read, close, lstat, readlink, getcwd, setgroups and the allocator) are served by a
//! small in-memory filesystem, and errno is per thread, as in the library.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr::null_mut;

#[path = "../libs/libbreenix-libc/src/pwd.rs"]
mod pwd;
#[path = "../libs/libbreenix-libc/src/grp.rs"]
mod grp;
#[path = "../libs/libbreenix-libc/src/libgen.rs"]
mod libgen;

mod pthread {
    use std::cell::Cell;
    thread_local!(static ERRNO: Cell<i32> = const { Cell::new(0) });
    pub(crate) fn set_errno(e: i32) {
        ERRNO.with(|v| v.set(e))
    }
    pub(crate) fn errno() -> i32 {
        ERRNO.with(|v| v.get())
    }
}

enum Node {
    File(Vec<u8>),
    Dir,
    Link(Vec<u8>),
}

thread_local! {
    static FS: RefCell<HashMap<Vec<u8>, Node>> = RefCell::new(HashMap::new());
    static OPEN: RefCell<HashMap<i32, (Vec<u8>, usize)>> = RefCell::new(HashMap::new());
    static CWD: RefCell<Vec<u8>> = RefCell::new(b"/".to_vec());
    static GROUPS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static FAIL_ALLOC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn reset(files: &[(&str, Node)]) {
    FS.with(|fs| {
        let mut fs = fs.borrow_mut();
        fs.clear();
        fs.insert(b"/".to_vec(), Node::Dir);
        for (path, node) in files {
            let node = match node {
                Node::File(b) => Node::File(b.clone()),
                Node::Dir => Node::Dir,
                Node::Link(t) => Node::Link(t.clone()),
            };
            fs.insert(path.as_bytes().to_vec(), node);
        }
    });
    CWD.with(|c| *c.borrow_mut() = b"/".to_vec());
}

unsafe fn path_of(p: *const u8) -> Vec<u8> {
    CStr::from_ptr(p as *const _).to_bytes().to_vec()
}

const ENOENT: i32 = 2;

pub unsafe fn open(path: *const u8, _flags: i32, _mode: u32) -> i32 {
    let path = path_of(path);
    let content = FS.with(|fs| match fs.borrow().get(&path) {
        Some(Node::File(b)) => Some(b.clone()),
        _ => None,
    });
    match content {
        Some(b) => OPEN.with(|o| {
            let mut o = o.borrow_mut();
            let fd = 3 + o.len() as i32;
            o.insert(fd, (b, 0));
            fd
        }),
        None => {
            pthread::set_errno(ENOENT);
            -1
        }
    }
}

/// Reads at most 7 bytes at a time, so the database reader must loop and grow.
pub unsafe fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    if FAIL_READ.with(|f| f.get()) {
        pthread::set_errno(5);
        return -1;
    }
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let (data, pos) = o.get_mut(&fd).expect("read of an open fd");
        let n = count.min(7).min(data.len() - *pos);
        std::ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buf, n);
        *pos += n;
        n as isize
    })
}

pub fn close(fd: i32) -> i32 {
    OPEN.with(|o| o.borrow_mut().remove(&fd));
    0
}

pub unsafe fn malloc(n: usize) -> *mut u8 {
    if FAIL_ALLOC.with(|f| f.get()) {
        pthread::set_errno(12);
        return null_mut();
    }
    libc_alloc(n)
}

unsafe fn libc_alloc(n: usize) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(n + 16, 16).unwrap();
    let p = std::alloc::alloc(layout);
    *(p as *mut usize) = n;
    p.add(16)
}

pub unsafe fn free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    let base = p.sub(16);
    let n = *(base as *const usize);
    std::alloc::dealloc(base, std::alloc::Layout::from_size_align(n + 16, 16).unwrap());
}

pub unsafe fn realloc(p: *mut u8, n: usize) -> *mut u8 {
    let q = libc_alloc(n);
    if !p.is_null() {
        let old = *(p.sub(16) as *const usize);
        std::ptr::copy_nonoverlapping(p, q, old.min(n));
        free(p);
    }
    q
}

pub unsafe fn getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    let cwd = CWD.with(|c| c.borrow().clone());
    assert!(cwd.len() < size);
    std::ptr::copy_nonoverlapping(cwd.as_ptr(), buf, cwd.len());
    *buf.add(cwd.len()) = 0;
    buf
}

/// st_mode at the arch's offset in struct stat, as libgen.rs reads it.
pub unsafe fn lstat(path: *const u8, st: *mut u8) -> i32 {
    let path = path_of(path);
    let mode = FS.with(|fs| match fs.borrow().get(&path) {
        Some(Node::File(_)) => Some(0o100644u32),
        Some(Node::Dir) => Some(0o040755),
        Some(Node::Link(_)) => Some(0o120777),
        None => None,
    });
    match mode {
        Some(m) => {
            let off = if cfg!(target_arch = "x86_64") { 24 } else { 16 };
            *(st.add(off) as *mut u32) = m;
            0
        }
        None => {
            pthread::set_errno(ENOENT);
            -1
        }
    }
}

pub unsafe fn readlink(path: *const u8, buf: *mut u8, size: usize) -> isize {
    let path = path_of(path);
    FS.with(|fs| match fs.borrow().get(&path) {
        Some(Node::Link(t)) => {
            let n = t.len().min(size);
            std::ptr::copy_nonoverlapping(t.as_ptr(), buf, n);
            n as isize
        }
        _ => {
            pthread::set_errno(22);
            -1
        }
    })
}

pub unsafe fn setgroups(n: usize, list: *const u32) -> i32 {
    GROUPS.with(|g| *g.borrow_mut() = std::slice::from_raw_parts(list, n).to_vec());
    0
}

// Only non-reentrant passwd callers share production storage. Hold this through
// reading their returned pointers; the reentrant, group and pathname cases run freely.
static PASSWD_STORAGE: std::sync::Mutex<()> = std::sync::Mutex::new(());

const PASSWD: &[u8] = b"root:x:0:0:root:/root:/bin/sh\n\
broken line without fields\n\
toolong:x:99999999999:0::/:/bin/sh\n\
wrb:x:1000:100:Ryan:/home/wrb:/bin/ash\n\
nobody:x:65534:65534:nobody:/nonexistent:/bin/false";

const GROUP: &[u8] = b"root:x:0:\n\
users:x:100:wrb,alice\n\
:bad\n\
wheel:x:10:alice,wrb\n";

fn databases() {
    reset(&[
        ("/etc", Node::Dir),
        ("/etc/passwd", Node::File(PASSWD.to_vec())),
        ("/etc/group", Node::File(GROUP.to_vec())),
    ]);
}

unsafe fn text(p: *const u8) -> String {
    String::from_utf8(path_of(p)).unwrap()
}

#[test]
fn getpwnam_and_getpwuid_find_entries_and_skip_malformed_lines() {
    let _storage = PASSWD_STORAGE.lock().unwrap();
    databases();
    unsafe {
        let name = CString::new("wrb").unwrap();
        let pw = &*(pwd::getpwnam(name.as_ptr() as *const u8) as *const [usize; 7]);
        let pw = pw.as_ptr() as *const u8;
        let field = |i: usize| text(*(pw.add(i * 8) as *const *const u8));
        assert_eq!(field(0), "wrb");
        assert_eq!(*(pw.add(16) as *const u32), 1000);
        assert_eq!(*(pw.add(20) as *const u32), 100);
        assert_eq!(field(3), "Ryan");
        assert_eq!(field(4), "/home/wrb");
        assert_eq!(field(5), "/bin/ash");

        let pw = pwd::getpwuid(65534) as *const u8;
        assert!(!pw.is_null());
        assert_eq!(text(*(pw as *const *const u8)), "nobody");
        // The last line has no newline.
        assert_eq!(text(*(pw.add(40) as *const *const u8)), "/bin/false");

        pthread::set_errno(0);
        assert!(pwd::getpwuid(4242).is_null());
        assert_eq!(pthread::errno(), 0, "not finding an entry is not an error");
        let missing = CString::new("toolong").unwrap();
        assert!(pwd::getpwnam(missing.as_ptr() as *const u8).is_null(), "a uid above 32 bits is malformed");
    }
}

#[test]
fn getpwuid_r_reports_erange_for_a_small_buffer() {
    databases();
    unsafe {
        let mut entry = [0usize; 7];
        let mut result: *mut pwd::Passwd = null_mut();
        let mut buf = [0u8; 8];
        let status = pwd::getpwuid_r(0, entry.as_mut_ptr() as *mut _, buf.as_mut_ptr(), buf.len(), &mut result);
        assert_eq!(status, 34);
        assert!(result.is_null());
        let mut big = [0u8; 256];
        let status = pwd::getpwuid_r(0, entry.as_mut_ptr() as *mut _, big.as_mut_ptr(), big.len(), &mut result);
        assert_eq!(status, 0);
        assert_eq!(result as usize, entry.as_ptr() as usize);
    }
}

#[test]
fn getpwent_walks_every_well_formed_entry() {
    let _storage = PASSWD_STORAGE.lock().unwrap();
    databases();
    unsafe {
        pwd::endpwent();
        let mut names = Vec::new();
        loop {
            let pw = pwd::getpwent();
            if pw.is_null() {
                break;
            }
            names.push(text(*(pw as *const *const u8)));
        }
        assert_eq!(names, ["root", "wrb", "nobody"]);
        pwd::setpwent();
        assert_eq!(text(*(pwd::getpwent() as *const *const u8)), "root");
        pwd::endpwent();
    }
}

#[test]
fn getgrgid_lists_members_and_getgrouplist_collects_groups() {
    databases();
    unsafe {
        let gr = grp::getgrgid(100) as *const u8;
        assert!(!gr.is_null());
        assert_eq!(text(*(gr as *const *const u8)), "users");
        let mem = *(gr.add(24) as *const *const *const u8);
        assert_eq!(text(*mem), "wrb");
        assert_eq!(text(*mem.add(1)), "alice");
        assert!((*mem.add(2)).is_null());

        let root = grp::getgrgid(0) as *const u8;
        assert!((*(*(root.add(24) as *const *const *const u8))).is_null(), "no members");

        let user = CString::new("wrb").unwrap();
        let mut groups = [0u32; 8];
        let mut n = 8;
        assert_eq!(grp::getgrouplist(user.as_ptr() as *const u8, 100, groups.as_mut_ptr(), &mut n), 2);
        assert_eq!(&groups[..2], &[100, 10]);
        let mut one = [0u32; 1];
        let mut n = 1;
        assert_eq!(grp::getgrouplist(user.as_ptr() as *const u8, 100, one.as_mut_ptr(), &mut n), -1);
        assert_eq!(n, 2, "the count needed");
        assert_eq!(grp::initgroups(user.as_ptr() as *const u8, 5), 0);
        assert_eq!(GROUPS.with(|g| g.borrow().clone()), vec![5, 100, 10]);
    }
}

#[test]
fn getgrnam_r_reports_erange_for_a_small_buffer() {
    databases();
    unsafe {
        let mut entry = [0usize; 4];
        let mut result: *mut grp::Group = null_mut();
        let mut buf = [0u8; 20];
        let name = CString::new("wheel").unwrap();
        let status = grp::getgrnam_r(name.as_ptr() as *const u8, entry.as_mut_ptr() as *mut _, buf.as_mut_ptr(),
                                     buf.len(), &mut result);
        assert_eq!(status, 34);
        assert!(result.is_null());
    }
}

unsafe fn call(f: unsafe extern "C" fn(*mut u8) -> *mut u8, input: Option<&str>) -> String {
    match input {
        None => text(f(null_mut())),
        Some(s) => {
            let mut owned = CString::new(s).unwrap().into_bytes_with_nul();
            text(f(owned.as_mut_ptr()))
        }
    }
}

#[test]
fn basename_follows_posix() {
    unsafe {
        for (input, want) in [
            (Some("/usr/lib"), "lib"),
            (Some("/usr/"), "usr"),
            (Some("usr"), "usr"),
            (Some("/"), "/"),
            (Some("///"), "/"),
            (Some("//usr//lib//"), "lib"),
            (Some(""), "."),
            (None, "."),
        ] {
            assert_eq!(call(libgen::basename, input), want, "basename({input:?})");
        }
    }
}

#[test]
fn dirname_follows_posix() {
    unsafe {
        for (input, want) in [
            (Some("/usr/lib"), "/usr"),
            (Some("/usr/"), "/"),
            (Some("usr"), "."),
            (Some("/"), "/"),
            (Some("///"), "/"),
            (Some("a/b//"), "a"),
            (Some("//usr//lib//"), "//usr"),
            (Some("/a/b/c.txt"), "/a/b"),
            (Some(""), "."),
            (None, "."),
        ] {
            assert_eq!(call(libgen::dirname, input), want, "dirname({input:?})");
        }
    }
}

unsafe fn realpath(path: &str) -> Result<String, i32> {
    let p = CString::new(path).unwrap();
    let r = libgen::realpath(p.as_ptr() as *const u8, null_mut());
    if r.is_null() {
        return Err(pthread::errno());
    }
    let s = text(r);
    free(r);
    Ok(s)
}

#[test]
fn realpath_resolves_dots_and_links() {
    reset(&[
        ("/a", Node::Dir),
        ("/a/b", Node::Dir),
        ("/a/b/f", Node::File(b"x".to_vec())),
        ("/a/up", Node::Link(b"..".to_vec())),
        ("/a/abs", Node::Link(b"/a/b".to_vec())),
        ("/a/rel", Node::Link(b"b/f".to_vec())),
        ("/a/loop", Node::Link(b"loop".to_vec())),
    ]);
    unsafe {
        assert_eq!(realpath("/a/./b/../b/f"), Ok("/a/b/f".into()));
        assert_eq!(realpath("/a/abs/f"), Ok("/a/b/f".into()));
        assert_eq!(realpath("/a/rel"), Ok("/a/b/f".into()));
        assert_eq!(realpath("/a/rel/"), Err(20));
        assert_eq!(realpath("/a/rel//"), Err(20));
        assert_eq!(realpath("/a/abs/"), Ok("/a/b".into()));
        assert_eq!(realpath("/a/up/a/b"), Ok("/a/b".into()));
        assert_eq!(realpath("/.."), Ok("/".into()));
        assert_eq!(realpath("//a//b/"), Ok("/a/b".into()));
        CWD.with(|c| *c.borrow_mut() = b"/a".to_vec());
        assert_eq!(realpath("b/f"), Ok("/a/b/f".into()));
        assert_eq!(realpath("."), Ok("/a".into()));
        assert_eq!(realpath("/a/missing"), Err(2));
        assert_eq!(realpath("/a/b/f/x"), Err(20), "a file followed by a component");
        assert_eq!(realpath("/a/loop"), Err(40));
        assert_eq!(realpath(""), Err(2));
    }
}

#[test]
fn group_database_failure_preserves_supplementary_groups() {
    reset(&[]);
    GROUPS.with(|g| *g.borrow_mut() = vec![42]);
    unsafe {
        let user = CString::new("wrb").unwrap();
        assert_eq!(grp::initgroups(user.as_ptr() as *const u8, 5), -1);
        assert_eq!(pthread::errno(), ENOENT);
        assert_eq!(GROUPS.with(|g| g.borrow().clone()), vec![42]);
    }
}

#[test]
fn group_database_read_and_allocation_failures_are_reported() {
    databases();
    unsafe {
        let user = CString::new("wrb").unwrap();
        let mut list = [0u32; 8];
        let mut n = 8;
        FAIL_READ.with(|f| f.set(true));
        assert_eq!(grp::getgrouplist(user.as_ptr() as *const u8, 5, list.as_mut_ptr(), &mut n), -1);
        assert_eq!(pthread::errno(), 5);
        FAIL_READ.with(|f| f.set(false));
        FAIL_ALLOC.with(|f| f.set(true));
        assert_eq!(grp::getgrouplist(user.as_ptr() as *const u8, 5, list.as_mut_ptr(), &mut n), -1);
        assert_eq!(pthread::errno(), 12);
        FAIL_ALLOC.with(|f| f.set(false));
    }
}

#[test]
fn initgroups_grows_past_sixty_four_entries() {
    let mut data = Vec::new();
    for gid in 1..100 { data.extend_from_slice(format!("g{gid}:x:{gid}:wrb\n").as_bytes()); }
    reset(&[("/etc/group", Node::File(data))]);
    unsafe {
        let user = CString::new("wrb").unwrap();
        assert_eq!(grp::initgroups(user.as_ptr() as *const u8, 0), 0);
        assert_eq!(GROUPS.with(|g| g.borrow().clone()), (0..100).collect::<Vec<u32>>());
    }
}
