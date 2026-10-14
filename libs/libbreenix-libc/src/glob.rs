//! glob and globfree (POSIX.1-2024 XSH glob; patterns per XCU 2.14.3, Patterns Used for
//! Filename Expansion). Flags are independent bits assigned by this implementation.
//!
//! The pattern is taken one `/`-separated component at a time. A component with no
//! unescaped `*`, `?` or `[` is appended as it stands (backslashes removed unless
//! GLOB_NOESCAPE) and checked for existence once the whole path is built. Any other
//! component is matched with fnmatch against each entry of the directory built so far,
//! with FNM_PERIOD, so a leading period is matched only by a period in the pattern.

use core::ptr::null_mut;

use crate::dirent::{closedir, opendir, readdir, sort_ptrs};
use crate::fnmatch::{matches, FNM_NOESCAPE, FNM_PERIOD};

extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn realloc(ptr: *mut u8, size: usize) -> *mut u8;
    fn free(ptr: *mut u8);
    fn stat(path: *const u8, buf: *mut u8) -> i32;
    fn lstat(path: *const u8, buf: *mut u8) -> i32;
    fn __errno_location() -> *mut i32;
}

// Independent flag bits in API declaration order; errors are successive positive codes.
const GLOB_ERR: i32 = 1 << 0;
const GLOB_MARK: i32 = 1 << 1;
const GLOB_NOSORT: i32 = 1 << 2;
const GLOB_DOOFFS: i32 = 1 << 3;
const GLOB_NOCHECK: i32 = 1 << 4;
const GLOB_APPEND: i32 = 1 << 5;
const GLOB_NOESCAPE: i32 = 1 << 6;
const GLOB_PERIOD: i32 = 1 << 7;
const GLOB_MAGCHAR: i32 = 1 << 8;
const GLOB_NOMAGIC: i32 = 1 << 9;

const GLOB_NOSPACE: i32 = 1;
const GLOB_ABORTED: i32 = 2;
const GLOB_NOMATCH: i32 = 3;

// Error numbers and stat encodings/offsets follow the Breenix kernel syscall ABI.
const ENOENT: i32 = 2;
const ENOTDIR: i32 = 20;

#[cfg(target_arch = "x86_64")]
const ST_MODE: usize = 24;
#[cfg(target_arch = "aarch64")]
const ST_MODE: usize = 16;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;

/// The POSIX members and our returned flags; no unused foreign-layout fields.
#[repr(C)]
pub struct Glob {
    gl_pathc: usize,
    gl_pathv: *mut *mut u8,
    gl_offs: usize,
    gl_flags: i32,
}

type ErrFunc = Option<unsafe extern "C" fn(*const u8, i32) -> i32>;

/// A NUL-terminated byte string in malloc'd storage.
struct Path {
    ptr: *mut u8,
    len: usize,
    cap: usize,
}

impl Path {
    fn new() -> Self {
        Path {
            ptr: null_mut(),
            len: 0,
            cap: 0,
        }
    }
    unsafe fn push(&mut self, b: &[u8]) -> Result<(), i32> {
        let need = self.len + b.len() + 1;
        if need > self.cap {
            let cap = need.max(self.cap * 2).max(64);
            let grown = realloc(self.ptr, cap);
            if grown.is_null() {
                return Err(GLOB_NOSPACE);
            }
            self.ptr = grown;
            self.cap = cap;
        }
        core::ptr::copy_nonoverlapping(b.as_ptr(), self.ptr.add(self.len), b.len());
        self.len += b.len();
        *self.ptr.add(self.len) = 0;
        Ok(())
    }
    unsafe fn truncate(&mut self, len: usize) {
        self.len = len;
        if !self.ptr.is_null() {
            *self.ptr.add(len) = 0;
        }
    }
    unsafe fn bytes(&self) -> &[u8] {
        if self.ptr.is_null() {
            &[]
        } else {
            core::slice::from_raw_parts(self.ptr, self.len)
        }
    }
}

impl Drop for Path {
    fn drop(&mut self) {
        unsafe { free(self.ptr) };
    }
}

/// A growing array of malloc'd C strings.
struct List {
    v: *mut *mut u8,
    len: usize,
    cap: usize,
}

impl List {
    unsafe fn push_copy(&mut self, b: &[u8]) -> Result<(), i32> {
        if self.len == self.cap {
            let cap = (self.cap * 2).max(16);
            let grown =
                realloc(self.v as *mut u8, cap * core::mem::size_of::<*mut u8>()) as *mut *mut u8;
            if grown.is_null() {
                return Err(GLOB_NOSPACE);
            }
            self.v = grown;
            self.cap = cap;
        }
        let s = malloc(b.len() + 1);
        if s.is_null() {
            return Err(GLOB_NOSPACE);
        }
        core::ptr::copy_nonoverlapping(b.as_ptr(), s, b.len());
        *s.add(b.len()) = 0;
        *self.v.add(self.len) = s;
        self.len += 1;
        Ok(())
    }
    unsafe fn release(&mut self) {
        for i in 0..self.len {
            free(*self.v.add(i));
        }
        free(self.v as *mut u8);
        self.v = null_mut();
        self.len = 0;
        self.cap = 0;
    }
}

struct Ctx {
    flags: i32,
    errfunc: ErrFunc,
    out: List,
}

fn has_magic(p: &[u8], noescape: bool) -> bool {
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            b'\\' if !noescape => i += 1,
            b'*' | b'?' | b'[' => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

unsafe fn mode_of(path: *const u8, follow: bool) -> Option<u32> {
    let mut st = [0u64; 32];
    let r = if follow {
        stat(path, st.as_mut_ptr() as *mut u8)
    } else {
        lstat(path, st.as_mut_ptr() as *mut u8)
    };
    (r == 0).then(|| core::ptr::read((st.as_ptr() as *const u8).add(ST_MODE) as *const u32))
}

/// Expand `rest` below the directory `path` names; `listed` says `path` came from a
/// directory listing and so is known to exist.
unsafe fn walk(ctx: &mut Ctx, path: &mut Path, rest: &[u8], listed: bool) -> Result<(), i32> {
    let mut rest = rest;
    while rest.first() == Some(&b'/') {
        path.push(b"/")?;
        rest = &rest[1..];
    }
    let noescape = ctx.flags & GLOB_NOESCAPE != 0;
    if rest.is_empty() {
        if path.len == 0 {
            return Ok(());
        }
        let need_dir = path.bytes().ends_with(b"/");
        if need_dir {
            match mode_of(path.ptr, true) {
                Some(m) if m & S_IFMT == S_IFDIR => {}
                Some(_) => return Ok(()),
                None => return directory_error(ctx, path.ptr),
            }
        } else if !listed && mode_of(path.ptr, false).is_none() {
            return directory_error(ctx, path.ptr);
        }
        return ctx.out.push_copy(path.bytes());
    }
    // A quoted slash is still a pathname separator, but consume its quoting byte
    // with it so the preceding component does not acquire a trailing backslash.
    let mut end = 0;
    let mut quoted_slash = false;
    while end < rest.len() {
        if rest[end] == b'/' {
            break;
        }
        if !noescape && rest[end] == b'\\' && end + 1 < rest.len() {
            if rest[end + 1] == b'/' {
                quoted_slash = true;
                break;
            }
            end += 1;
        }
        end += 1;
    }
    let (comp, mut after) = rest.split_at(end);
    if quoted_slash {
        after = &after[1..];
    }
    let mark = path.len;
    if !has_magic(comp, noescape) {
        let mut i = 0;
        while i < comp.len() {
            if comp[i] == b'\\' && !noescape && i + 1 < comp.len() {
                i += 1;
            }
            path.push(&comp[i..i + 1])?;
            i += 1;
        }
        let r = walk(ctx, path, after, false);
        path.truncate(mark);
        return r;
    }
    let dir_name: *const u8 = if path.len == 0 {
        b".\0".as_ptr()
    } else {
        path.ptr
    };
    let d = opendir(dir_name);
    if d.is_null() {
        return directory_error(ctx, dir_name);
    }
    let mut fn_flags = 0;
    if ctx.flags & GLOB_PERIOD == 0 {
        fn_flags |= FNM_PERIOD;
    }
    if noescape {
        fn_flags |= FNM_NOESCAPE;
    }
    let saved = *__errno_location();
    *__errno_location() = 0;
    let result = loop {
        let e = readdir(d);
        if e.is_null() {
            let err = *__errno_location();
            break if err != 0 {
                directory_error(ctx, if mark == 0 { b".\0".as_ptr() } else { path.ptr })
            } else {
                Ok(())
            };
        }
        let name =
            core::ffi::CStr::from_ptr((*e).d_name.as_ptr() as *const core::ffi::c_char).to_bytes();
        if !matches(comp, name, fn_flags) {
            continue;
        }
        if let Err(code) = path.push(name) {
            break Err(code);
        }
        let r = walk(ctx, path, after, true);
        path.truncate(mark);
        if r.is_err() {
            break r;
        }
        *__errno_location() = 0;
    };
    closedir(d);
    *__errno_location() = saved;
    result
}

/// A directory that could not be opened or read: ignored if it does not exist or is not
/// a directory, else reported to errfunc; GLOB_ERR or a nonzero errfunc result stops.
unsafe fn directory_error(ctx: &mut Ctx, dir: *const u8) -> Result<(), i32> {
    let e = *__errno_location();
    if e == ENOENT || e == ENOTDIR {
        return Ok(());
    }
    let stop = match ctx.errfunc {
        Some(f) => f(dir, e) != 0,
        None => false,
    };
    if stop || ctx.flags & GLOB_ERR != 0 {
        Err(GLOB_ABORTED)
    } else {
        Ok(())
    }
}

unsafe fn byte_cmp(a: *const u8, b: *const u8) -> i32 {
    let mut i = 0;
    loop {
        let (x, y) = (*a.add(i), *b.add(i));
        if x != y || x == 0 {
            return x as i32 - y as i32;
        }
        i += 1;
    }
}

/// Append a `/` to each new path that names a directory.
unsafe fn mark_dirs(list: &mut List) -> Result<(), i32> {
    for i in 0..list.len {
        let s = *list.v.add(i);
        let n = core::ffi::CStr::from_ptr(s as *const core::ffi::c_char)
            .to_bytes()
            .len();
        if n > 0 && *s.add(n - 1) == b'/' {
            continue;
        }
        if !matches!(mode_of(s, true), Some(m) if m & S_IFMT == S_IFDIR) {
            continue;
        }
        let grown = realloc(s, n + 2);
        if grown.is_null() {
            return Err(GLOB_NOSPACE);
        }
        *grown.add(n) = b'/';
        *grown.add(n + 1) = 0;
        *list.v.add(i) = grown;
    }
    Ok(())
}

/// Put the new paths into `pglob`, after any it holds under GLOB_APPEND.
unsafe fn store(pglob: &mut Glob, flags: i32, new: &mut List) -> Result<(), i32> {
    let offs = if flags & GLOB_DOOFFS != 0 {
        pglob.gl_offs
    } else {
        0
    };
    let (old_v, old_c) = if flags & GLOB_APPEND != 0 {
        (pglob.gl_pathv, pglob.gl_pathc)
    } else {
        (null_mut(), 0)
    };
    let total = offs
        .checked_add(old_c)
        .and_then(|n| n.checked_add(new.len))
        .and_then(|n| n.checked_add(1))
        .and_then(|n| n.checked_mul(core::mem::size_of::<*mut u8>()))
        .ok_or(GLOB_NOSPACE)?;
    let v = realloc(old_v as *mut u8, total) as *mut *mut u8;
    if v.is_null() {
        return Err(GLOB_NOSPACE);
    }
    if old_v.is_null() {
        for i in 0..offs {
            *v.add(i) = null_mut();
        }
    }
    for i in 0..new.len {
        *v.add(offs + old_c + i) = *new.v.add(i);
    }
    *v.add(offs + old_c + new.len) = null_mut();
    pglob.gl_pathv = v;
    pglob.gl_pathc = old_c + new.len;
    free(new.v as *mut u8);
    new.v = null_mut();
    new.len = 0;
    new.cap = 0;
    Ok(())
}

/// glob - the pathnames that match `pattern`.
#[no_mangle]
pub unsafe extern "C" fn glob(
    pattern: *const u8,
    flags: i32,
    errfunc: ErrFunc,
    pglob: *mut Glob,
) -> i32 {
    let g = &mut *pglob;
    if flags & GLOB_APPEND == 0 {
        g.gl_pathc = 0;
        g.gl_pathv = null_mut();
    }
    let p = core::ffi::CStr::from_ptr(pattern as *const core::ffi::c_char).to_bytes();
    let noescape = flags & GLOB_NOESCAPE != 0;
    let magic = has_magic(p, noescape);
    let mut ctx = Ctx {
        flags,
        errfunc,
        out: List {
            v: null_mut(),
            len: 0,
            cap: 0,
        },
    };
    let mut path = Path::new();
    let mut result = walk(&mut ctx, &mut path, p, false);
    drop(path);
    if result.is_ok() && ctx.flags & GLOB_MARK != 0 {
        result = mark_dirs(&mut ctx.out);
    }
    if result.is_ok()
        && ctx.out.len == 0
        && (flags & GLOB_NOCHECK != 0 || (flags & GLOB_NOMAGIC != 0 && !magic))
    {
        result = ctx.out.push_copy(p);
    }
    if result.is_ok() && flags & GLOB_NOSORT == 0 {
        sort_ptrs(ctx.out.v, ctx.out.len, |a, b| byte_cmp(*a, *b));
    }
    let code = match result {
        Err(code) => code,
        Ok(()) if ctx.out.len == 0 => GLOB_NOMATCH,
        Ok(()) => 0,
    };
    // POSIX glob retains paths scanned before GLOB_ABORTED or GLOB_NOSPACE.
    if store(g, flags, &mut ctx.out).is_err() {
        ctx.out.release();
        g.gl_flags = flags;
        return GLOB_NOSPACE;
    }
    g.gl_flags = if magic { flags | GLOB_MAGCHAR } else { flags };
    code
}

/// globfree - release what glob stored in `pglob`.
#[no_mangle]
pub unsafe extern "C" fn globfree(pglob: *mut Glob) {
    let g = &mut *pglob;
    if g.gl_pathv.is_null() {
        g.gl_pathc = 0;
        return;
    }
    let offs = if g.gl_flags & GLOB_DOOFFS != 0 {
        g.gl_offs
    } else {
        0
    };
    for i in 0..g.gl_pathc {
        free(*g.gl_pathv.add(offs + i));
    }
    free(g.gl_pathv as *mut u8);
    g.gl_pathv = null_mut();
    g.gl_pathc = 0;
}
