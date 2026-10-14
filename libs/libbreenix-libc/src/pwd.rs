//! The user database (POSIX.1-2024 <pwd.h>) over /etc/passwd, whose lines are
//! `name:password:uid:gid:gecos:home:shell`. Malformed lines are skipped.
//!
//! The `_r` functions copy the entry's strings into the caller's buffer and fail with
//! ERANGE when it is too small; not finding an entry is success with a null result.
//! The others return storage of this module that the next call overwrites.
//!
//! Also the line reader grp.rs shares.

use core::ptr::{addr_of_mut, null_mut};

const ENOMEM: i32 = 12;
const ERANGE: i32 = 34;
const O_RDONLY: i32 = 0;
const O_CLOEXEC: i32 = 0o2000000;

/// A whole database file read into memory, and a cursor over its lines.
pub(crate) struct Database {
    data: *mut u8,
    len: usize,
    pos: usize,
}

impl Database {
    pub(crate) const fn empty() -> Self {
        Database { data: null_mut(), len: 0, pos: 0 }
    }

    /// Read `path` (NUL-terminated). Err(errno) when it cannot be read.
    pub(crate) unsafe fn open(path: &[u8]) -> Result<Self, i32> {
        let fd = crate::open(path.as_ptr(), O_RDONLY | O_CLOEXEC, 0);
        if fd < 0 {
            return Err(crate::pthread::errno());
        }
        let mut cap = 1024usize;
        let mut data = crate::malloc(cap);
        let mut len = 0usize;
        let result = loop {
            if data.is_null() {
                break Err(ENOMEM);
            }
            if len == cap {
                let bigger = crate::realloc(data, cap * 2);
                if bigger.is_null() {
                    crate::free(data);
                    data = null_mut();
                    continue;
                }
                data = bigger;
                cap *= 2;
            }
            let n = crate::read(fd, data.add(len), cap - len);
            if n < 0 {
                let e = crate::pthread::errno();
                crate::free(data);
                break Err(e);
            }
            if n == 0 {
                break Ok(Database { data, len, pos: 0 });
            }
            len += n as usize;
        };
        crate::close(fd);
        result
    }

    pub(crate) fn rewind(&mut self) {
        self.pos = 0;
    }

    pub(crate) fn is_open(&self) -> bool {
        !self.data.is_null()
    }

    /// The next line, without its newline.
    pub(crate) fn next_line(&mut self) -> Option<&[u8]> {
        if self.data.is_null() || self.pos >= self.len {
            return None;
        }
        let all = unsafe { core::slice::from_raw_parts(self.data, self.len) };
        let rest = &all[self.pos..];
        let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        self.pos += end + 1;
        Some(&rest[..end])
    }

    pub(crate) unsafe fn close(&mut self) {
        if !self.data.is_null() {
            crate::free(self.data);
        }
        *self = Database::empty();
    }
}

/// The `:`-separated fields of a line, exactly `N` of them or None.
pub(crate) fn fields<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    let mut out: [&[u8]; N] = [&[]; N];
    let mut parts = line.split(|&b| b == b':');
    for slot in out.iter_mut() {
        *slot = parts.next()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

/// A decimal ID of at most 32 bits.
pub(crate) fn parse_id(text: &[u8]) -> Option<u32> {
    if text.is_empty() || text.len() > 10 {
        return None;
    }
    let mut value: u64 = 0;
    for &b in text {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value * 10 + (b - b'0') as u64;
    }
    u32::try_from(value).ok()
}

/// Copies strings and pointer arrays into a caller's buffer.
pub(crate) struct Packer {
    buf: *mut u8,
    cap: usize,
    used: usize,
}

impl Packer {
    pub(crate) fn new(buf: *mut u8, cap: usize) -> Self {
        Packer { buf, cap, used: 0 }
    }

    /// A NUL-terminated copy of `text`, or None when it does not fit.
    pub(crate) unsafe fn string(&mut self, text: &[u8]) -> Option<*mut u8> {
        if self.cap - self.used < text.len() + 1 {
            return None;
        }
        let at = self.buf.add(self.used);
        core::ptr::copy_nonoverlapping(text.as_ptr(), at, text.len());
        *at.add(text.len()) = 0;
        self.used += text.len() + 1;
        Some(at)
    }

    /// Room for `count` pointers, aligned for them.
    pub(crate) unsafe fn pointers(&mut self, count: usize) -> Option<*mut *mut u8> {
        let align = core::mem::align_of::<*mut u8>();
        let start = (self.buf as usize + self.used).next_multiple_of(align) - self.buf as usize;
        let bytes = count.checked_mul(core::mem::size_of::<*mut u8>())?;
        if start > self.cap || self.cap - start < bytes {
            return None;
        }
        self.used = start + bytes;
        Some(self.buf.add(start) as *mut *mut u8)
    }
}

#[repr(C)]
pub struct Passwd {
    pw_name: *mut u8,
    pw_passwd: *mut u8,
    pw_uid: u32,
    pw_gid: u32,
    pw_gecos: *mut u8,
    pw_dir: *mut u8,
    pw_shell: *mut u8,
}

impl Passwd {
    const fn empty() -> Self {
        Passwd { pw_name: null_mut(), pw_passwd: null_mut(), pw_uid: 0, pw_gid: 0,
                 pw_gecos: null_mut(), pw_dir: null_mut(), pw_shell: null_mut() }
    }
}

/// Fill `pwd` from a parsed line. Err(ERANGE) when the strings do not fit.
unsafe fn fill(f: &[&[u8]; 7], uid: u32, gid: u32, pwd: *mut Passwd, buf: *mut u8, len: usize) -> Result<(), i32> {
    let mut packer = Packer::new(buf, len);
    let mut next = |text| packer.string(text).ok_or(ERANGE);
    let entry = Passwd {
        pw_name: next(f[0])?,
        pw_passwd: next(f[1])?,
        pw_uid: uid,
        pw_gid: gid,
        pw_gecos: next(f[4])?,
        pw_dir: next(f[5])?,
        pw_shell: next(f[6])?,
    };
    pwd.write(entry);
    Ok(())
}

/// The fields and IDs of a well-formed line.
fn parse(line: &[u8]) -> Option<([&[u8]; 7], u32, u32)> {
    let f = fields::<7>(line)?;
    Some((f, parse_id(f[2])?, parse_id(f[3])?))
}

/// Search /etc/passwd for the first entry `want` accepts.
unsafe fn lookup(
    want: &dyn Fn(&[u8], u32) -> bool,
    pwd: *mut Passwd,
    buf: *mut u8,
    len: usize,
    result: *mut *mut Passwd,
) -> i32 {
    *result = null_mut();
    let mut db = match Database::open(b"/etc/passwd\0") {
        Ok(db) => db,
        Err(e) => return e,
    };
    let mut status = 0;
    while let Some(line) = db.next_line() {
        if let Some((f, uid, gid)) = parse(line) {
            if want(f[0], uid) {
                match fill(&f, uid, gid, pwd, buf, len) {
                    Ok(()) => *result = pwd,
                    Err(e) => status = e,
                }
                break;
            }
        }
    }
    db.close();
    status
}

unsafe fn c_bytes<'a>(s: *const u8) -> &'a [u8] {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(s, n)
}

#[no_mangle]
pub unsafe extern "C" fn getpwnam_r(
    name: *const u8, pwd: *mut Passwd, buf: *mut u8, len: usize, result: *mut *mut Passwd,
) -> i32 {
    let name = c_bytes(name);
    lookup(&|n, _| n == name, pwd, buf, len, result)
}

#[no_mangle]
pub unsafe extern "C" fn getpwuid_r(
    uid: u32, pwd: *mut Passwd, buf: *mut u8, len: usize, result: *mut *mut Passwd,
) -> i32 {
    lookup(&|_, u| u == uid, pwd, buf, len, result)
}

static mut ENTRY: Passwd = Passwd::empty();
static mut ENTRY_BUF: [u8; 1024] = [0; 1024];
static mut ENUMERATION: Database = Database::empty();

/// Return the static entry, or null with errno set when the lookup failed.
unsafe fn static_result(status: i32, result: *mut Passwd) -> *mut Passwd {
    if status != 0 {
        crate::pthread::set_errno(status);
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn getpwnam(name: *const u8) -> *mut Passwd {
    let mut result = null_mut();
    let status = getpwnam_r(name, addr_of_mut!(ENTRY), addr_of_mut!(ENTRY_BUF) as *mut u8, 1024, &mut result);
    static_result(status, result)
}

#[no_mangle]
pub unsafe extern "C" fn getpwuid(uid: u32) -> *mut Passwd {
    let mut result = null_mut();
    let status = getpwuid_r(uid, addr_of_mut!(ENTRY), addr_of_mut!(ENTRY_BUF) as *mut u8, 1024, &mut result);
    static_result(status, result)
}

#[no_mangle]
pub unsafe extern "C" fn setpwent() {
    let db = &mut *addr_of_mut!(ENUMERATION);
    if db.is_open() {
        db.rewind();
    }
}

#[no_mangle]
pub unsafe extern "C" fn endpwent() {
    (*addr_of_mut!(ENUMERATION)).close();
}

#[no_mangle]
pub unsafe extern "C" fn getpwent() -> *mut Passwd {
    let db = &mut *addr_of_mut!(ENUMERATION);
    if !db.is_open() {
        match Database::open(b"/etc/passwd\0") {
            Ok(opened) => *db = opened,
            Err(e) => {
                crate::pthread::set_errno(e);
                return null_mut();
            }
        }
    }
    while let Some(line) = db.next_line() {
        if let Some((f, uid, gid)) = parse(line) {
            return match fill(&f, uid, gid, addr_of_mut!(ENTRY), addr_of_mut!(ENTRY_BUF) as *mut u8, 1024) {
                Ok(()) => addr_of_mut!(ENTRY),
                Err(e) => static_result(e, null_mut()),
            };
        }
    }
    null_mut()
}
