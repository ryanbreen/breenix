//! The process environment (POSIX.1-2024 XBD 8.1; XSH getenv, setenv, unsetenv, putenv).
//!
//! `environ` starts as the array the kernel placed on the initial stack. The first change
//! copies it into an array this module allocates, so the startup array is never written.
//! `setenv` stores a "name=value" string it allocates; `putenv` stores the caller's own
//! string, so later changes to that string show through `getenv`. A string `setenv`
//! allocated is freed once it is replaced or removed, which invalidates a pointer `getenv`
//! returned for it, as XSH getenv permits; a `putenv` string is never freed here.
//!
//! None of these functions is required to be thread-safe (XSH 2.9.1). Rust std serializes
//! its own calls with its environment lock.

use core::ptr::null_mut;

extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn free(ptr: *mut u8);
    fn __errno_location() -> *mut i32;
}

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;

/// `char **environ`: the startup array until the first change, then an array owned here.
#[no_mangle]
pub static mut environ: *mut *mut u8 = null_mut();

/// The array this module allocated, and how many pointers it holds.
static mut OWNED: *mut *mut u8 = null_mut();
static mut OWNED_CAP: usize = 0;

/// The strings `setenv` allocated that `environ` still holds, and the array's capacity.
static mut ALLOCATED: *mut *mut u8 = null_mut();
static mut ALLOCATED_LEN: usize = 0;
static mut ALLOCATED_CAP: usize = 0;

unsafe fn set_errno(e: i32) {
    *__errno_location() = e;
}

unsafe fn c_len(s: *const u8) -> usize {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n
}

unsafe fn count() -> usize {
    let env = environ;
    if env.is_null() {
        return 0;
    }
    let mut n = 0;
    while !(*env.add(n)).is_null() {
        n += 1;
    }
    n
}

/// Does `entry` define `name` (the bytes before its first `=` equal `name`)?
unsafe fn defines(entry: *const u8, name: &[u8]) -> bool {
    for (i, &b) in name.iter().enumerate() {
        if *entry.add(i) != b {
            return false;
        }
    }
    *entry.add(name.len()) == b'='
}

/// The index of the first entry that defines `name`.
unsafe fn find(name: &[u8]) -> Option<usize> {
    let env = environ;
    if env.is_null() {
        return None;
    }
    let mut i = 0;
    loop {
        let entry = *env.add(i);
        if entry.is_null() {
            return None;
        }
        if defines(entry, name) {
            return Some(i);
        }
        i += 1;
    }
}

/// Make `environ` an array owned here with room for `extra` more entries. Sets ENOMEM and
/// returns false when no memory is left; `environ` is then unchanged.
unsafe fn own(extra: usize) -> bool {
    let n = count();
    let Some(need) = n.checked_add(extra).and_then(|v| v.checked_add(1)) else {
        set_errno(ENOMEM);
        return false;
    };
    if environ == OWNED && !OWNED.is_null() && need <= OWNED_CAP {
        return true;
    }
    let cap = need.max(n.saturating_mul(2)).max(16);
    let Some(bytes) = cap.checked_mul(core::mem::size_of::<*mut u8>()) else {
        set_errno(ENOMEM);
        return false;
    };
    let array = malloc(bytes) as *mut *mut u8;
    if array.is_null() {
        set_errno(ENOMEM);
        return false;
    }
    for i in 0..n {
        *array.add(i) = *environ.add(i);
    }
    *array.add(n) = null_mut();
    let old = OWNED;
    OWNED = array;
    OWNED_CAP = cap;
    environ = array;
    if !old.is_null() {
        free(old as *mut u8);
    }
    true
}

/// Make room to record one more allocated string. Sets ENOMEM and returns false when no
/// memory is left.
unsafe fn reserve_allocated() -> bool {
    if ALLOCATED_LEN < ALLOCATED_CAP {
        return true;
    }
    let cap = ALLOCATED_CAP.saturating_mul(2).max(16);
    let Some(bytes) = cap.checked_mul(core::mem::size_of::<*mut u8>()) else {
        set_errno(ENOMEM);
        return false;
    };
    let array = malloc(bytes) as *mut *mut u8;
    if array.is_null() {
        set_errno(ENOMEM);
        return false;
    }
    for i in 0..ALLOCATED_LEN {
        *array.add(i) = *ALLOCATED.add(i);
    }
    let old = ALLOCATED;
    ALLOCATED = array;
    ALLOCATED_CAP = cap;
    if !old.is_null() {
        free(old as *mut u8);
    }
    true
}

/// Free `entry`, which `environ` no longer holds, if `setenv` allocated it.
unsafe fn release(entry: *mut u8) {
    for i in 0..ALLOCATED_LEN {
        if *ALLOCATED.add(i) == entry {
            ALLOCATED_LEN -= 1;
            *ALLOCATED.add(i) = *ALLOCATED.add(ALLOCATED_LEN);
            free(entry);
            return;
        }
    }
}

/// A name `setenv` and `unsetenv` accept: not null, not empty, no `=`.
unsafe fn valid_name<'a>(name: *const u8) -> Option<&'a [u8]> {
    if name.is_null() {
        return None;
    }
    let bytes = core::slice::from_raw_parts(name, c_len(name));
    if bytes.is_empty() || bytes.contains(&b'=') {
        return None;
    }
    Some(bytes)
}

/// Store `entry` for `name`: replace the first definition or append one.
unsafe fn put(name: &[u8], entry: *mut u8) -> bool {
    match find(name) {
        Some(i) => {
            if !own(0) {
                return false;
            }
            let old = *environ.add(i);
            *environ.add(i) = entry;
            if old != entry {
                release(old);
            }
        }
        None => {
            if !own(1) {
                return false;
            }
            let n = count();
            *environ.add(n) = entry;
            *environ.add(n + 1) = null_mut();
        }
    }
    true
}

/// Remove every definition of `name`.
unsafe fn remove(name: &[u8]) -> bool {
    if find(name).is_none() {
        return true;
    }
    if !own(0) {
        return false;
    }
    let env = environ;
    let mut keep = 0;
    let mut i = 0;
    loop {
        let entry = *env.add(i);
        if entry.is_null() {
            break;
        }
        if defines(entry, name) {
            release(entry);
        } else {
            *env.add(keep) = entry;
            keep += 1;
        }
        i += 1;
    }
    *env.add(keep) = null_mut();
    true
}

/// The value of `name` for the other modules of the library.
pub(crate) fn getenv_bytes(name: &[u8]) -> Option<&'static [u8]> {
    if name.is_empty() || name.contains(&b'=') {
        return None;
    }
    unsafe {
        let i = find(name)?;
        let value = (*environ.add(i)).add(name.len() + 1);
        Some(core::slice::from_raw_parts(value, c_len(value)))
    }
}

/// getenv - the value of an environment variable, or null.
#[no_mangle]
pub unsafe extern "C" fn getenv(name: *const u8) -> *mut u8 {
    if name.is_null() {
        return null_mut();
    }
    let bytes = core::slice::from_raw_parts(name, c_len(name));
    if bytes.is_empty() || bytes.contains(&b'=') {
        return null_mut();
    }
    match find(bytes) {
        Some(i) => (*environ.add(i)).add(bytes.len() + 1),
        None => null_mut(),
    }
}

/// setenv - define `name` as a copy of "name=value", replacing an existing definition only
/// when `overwrite` is nonzero.
#[no_mangle]
pub unsafe extern "C" fn setenv(name: *const u8, value: *const u8, overwrite: i32) -> i32 {
    let Some(name) = valid_name(name) else {
        set_errno(EINVAL);
        return -1;
    };
    if value.is_null() {
        set_errno(EINVAL);
        return -1;
    }
    if overwrite == 0 && find(name).is_some() {
        return 0;
    }
    let value_len = c_len(value);
    let Some(size) = name.len().checked_add(value_len).and_then(|n| n.checked_add(2)) else {
        set_errno(ENOMEM);
        return -1;
    };
    if !reserve_allocated() {
        return -1;
    }
    let entry = malloc(size);
    if entry.is_null() {
        set_errno(ENOMEM);
        return -1;
    }
    core::ptr::copy_nonoverlapping(name.as_ptr(), entry, name.len());
    *entry.add(name.len()) = b'=';
    core::ptr::copy_nonoverlapping(value, entry.add(name.len() + 1), value_len);
    *entry.add(size - 1) = 0;
    if !put(name, entry) {
        free(entry);
        return -1;
    }
    *ALLOCATED.add(ALLOCATED_LEN) = entry;
    ALLOCATED_LEN += 1;
    0
}

/// unsetenv - remove every definition of `name`.
#[no_mangle]
pub unsafe extern "C" fn unsetenv(name: *const u8) -> i32 {
    let Some(name) = valid_name(name) else {
        set_errno(EINVAL);
        return -1;
    };
    if remove(name) {
        0
    } else {
        -1
    }
}

/// putenv - make the caller's "name=value" string itself part of the environment. A string
/// with no `=` removes that name, as on Linux.
#[no_mangle]
pub unsafe extern "C" fn putenv(string: *mut u8) -> i32 {
    if string.is_null() {
        set_errno(EINVAL);
        return -1;
    }
    let bytes = core::slice::from_raw_parts(string as *const u8, c_len(string));
    let ok = match bytes.iter().position(|&b| b == b'=') {
        Some(0) => {
            set_errno(EINVAL);
            return -1;
        }
        Some(eq) => put(&bytes[..eq], string),
        None => remove(bytes),
    };
    if ok {
        0
    } else {
        -1
    }
}

/// clearenv - remove every definition (a Linux and BSD extension).
#[no_mangle]
pub unsafe extern "C" fn clearenv() -> i32 {
    for i in 0..ALLOCATED_LEN {
        free(*ALLOCATED.add(i));
    }
    ALLOCATED_LEN = 0;
    if !OWNED.is_null() {
        *OWNED = null_mut();
        environ = OWNED;
    } else {
        environ = null_mut();
    }
    0
}
