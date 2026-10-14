//! Pathname functions: POSIX.1-2024 basename and dirname (<libgen.h>) and realpath
//! (<stdlib.h>).

use core::ptr::{addr_of_mut, null_mut};

const ENOENT: i32 = 2;
const ENOMEM: i32 = 12;
const ENOTDIR: i32 = 20;
const EINVAL: i32 = 22;
const ENAMETOOLONG: i32 = 36;
const ELOOP: i32 = 40;

const PATH_MAX: usize = 4096;
const SYMLOOP_MAX: usize = 40;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

/// "." for a null or empty argument, in writable storage of this module.
static mut DOT: [u8; 2] = *b".\0";

unsafe fn c_len(s: *const u8) -> usize {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n
}

/// basename: the final component of `path`, with trailing slashes removed by writing
/// over them; "/" for a path of only slashes and "." for a null or empty path.
#[no_mangle]
pub unsafe extern "C" fn basename(path: *mut u8) -> *mut u8 {
    if path.is_null() || *path == 0 {
        return addr_of_mut!(DOT) as *mut u8;
    }
    let mut end = c_len(path);
    while end > 1 && *path.add(end - 1) == b'/' {
        end -= 1;
    }
    *path.add(end) = 0;
    let mut start = end;
    while start > 0 && *path.add(start - 1) != b'/' {
        start -= 1;
    }
    // A path of only slashes keeps one.
    if start == end {
        return path;
    }
    path.add(start)
}

/// dirname: everything before the final component, without trailing slashes; "." when
/// there is no slash and "/" when only the root remains.
#[no_mangle]
pub unsafe extern "C" fn dirname(path: *mut u8) -> *mut u8 {
    if path.is_null() || *path == 0 {
        return addr_of_mut!(DOT) as *mut u8;
    }
    let mut end = c_len(path);
    // Trailing slashes, then the final component, then the slashes before it.
    while end > 1 && *path.add(end - 1) == b'/' {
        end -= 1;
    }
    while end > 0 && *path.add(end - 1) != b'/' {
        end -= 1;
    }
    if end == 0 {
        return addr_of_mut!(DOT) as *mut u8;
    }
    while end > 1 && *path.add(end - 1) == b'/' {
        end -= 1;
    }
    *path.add(end) = 0;
    path
}

/// The st_mode of `path` without following a final symbolic link.
unsafe fn lstat_mode(path: *const u8) -> Result<u32, i32> {
    let mut st = [0u64; 18];
    if crate::lstat(path, st.as_mut_ptr() as *mut u8) != 0 {
        return Err(crate::pthread::errno());
    }
    let bytes = st.as_ptr() as *const u8;
    #[cfg(target_arch = "aarch64")]
    let mode = *(bytes.add(16) as *const u32);
    #[cfg(target_arch = "x86_64")]
    let mode = *(bytes.add(24) as *const u32);
    Ok(mode)
}

/// Resolve `path` into `out` (NUL-terminated): absolute, with no ".", ".." or symbolic
/// link components (POSIX.1-2024 realpath).
unsafe fn resolve(path: &[u8], out: &mut [u8; PATH_MAX]) -> Result<(), i32> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    // `out[..len]` is the resolved prefix, "" standing for the root.
    let mut len = 0usize;
    if path[0] != b'/' {
        if crate::getcwd(out.as_mut_ptr(), PATH_MAX).is_null() {
            return Err(crate::pthread::errno());
        }
        len = c_len(out.as_ptr());
        if len == 1 {
            len = 0;
        }
    }
    // What is left to resolve sits at the end of `rest`.
    let mut rest = [0u8; PATH_MAX];
    if path.len() > PATH_MAX {
        return Err(ENAMETOOLONG);
    }
    let mut start = PATH_MAX - path.len();
    rest[start..].copy_from_slice(path);
    let mut links = 0usize;
    while start < PATH_MAX {
        let pending = &rest[start..];
        let n = pending.iter().position(|&b| b == b'/').unwrap_or(pending.len());
        let more = n < pending.len();
        let mut name = [0u8; 256];
        if n >= name.len() {
            return Err(ENAMETOOLONG);
        }
        name[..n].copy_from_slice(&pending[..n]);
        start += n + more as usize;
        let name = &name[..n];
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            while len > 0 && out[len - 1] != b'/' {
                len -= 1;
            }
            len = len.saturating_sub(1);
            continue;
        }
        if len + 1 + name.len() + 1 > PATH_MAX {
            return Err(ENAMETOOLONG);
        }
        let before = len;
        out[len] = b'/';
        out[len + 1..len + 1 + name.len()].copy_from_slice(name);
        len += 1 + name.len();
        out[len] = 0;
        let mode = lstat_mode(out.as_ptr())?;
        if mode & S_IFMT == S_IFLNK {
            links += 1;
            if links > SYMLOOP_MAX {
                return Err(ELOOP);
            }
            let mut target = [0u8; PATH_MAX];
            let t = crate::readlink(out.as_ptr(), target.as_mut_ptr(), PATH_MAX);
            if t < 0 {
                return Err(crate::pthread::errno());
            }
            let t = t as usize;
            let remaining = PATH_MAX - start;
            // rest = target, then "/" and what remained, if anything did.
            let joint = (remaining > 0) as usize;
            if t + joint + remaining > PATH_MAX {
                return Err(ENAMETOOLONG);
            }
            let new_start = PATH_MAX - remaining - joint - t;
            if joint == 1 {
                rest[new_start + t] = b'/';
            }
            rest[new_start..new_start + t].copy_from_slice(&target[..t]);
            start = new_start;
            len = if t > 0 && target[0] == b'/' { 0 } else { before };
        } else if more && mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
    }
    if len == 0 {
        out[0] = b'/';
        len = 1;
    }
    out[len] = 0;
    Ok(())
}

#[no_mangle]
pub unsafe extern "C" fn realpath(path: *const u8, resolved: *mut u8) -> *mut u8 {
    if path.is_null() {
        crate::pthread::set_errno(EINVAL);
        return null_mut();
    }
    let mut out = [0u8; PATH_MAX];
    if let Err(e) = resolve(core::slice::from_raw_parts(path, c_len(path)), &mut out) {
        crate::pthread::set_errno(e);
        return null_mut();
    }
    let n = c_len(out.as_ptr()) + 1;
    let dest = if resolved.is_null() { crate::malloc(n) } else { resolved };
    if dest.is_null() {
        crate::pthread::set_errno(ENOMEM);
        return null_mut();
    }
    core::ptr::copy_nonoverlapping(out.as_ptr(), dest, n);
    dest
}
