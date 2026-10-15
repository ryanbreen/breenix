//! Clean-room implementation of POSIX.1-2024 XSH posix_spawn and spawn.h.
//! The child uses only stack storage and raw syscalls: no inherited libc locks,
//! allocation, atfork callbacks, or shell fallback. Errors after fork exit 127,
//! as permitted by XSH posix_spawn; the parent returns the waitable child's PID.

use core::ptr::{null, null_mut};
use libbreenix::{raw, syscall::nr};

const RESETIDS: i16 = 1;
const SETPGROUP: i16 = 2;
const SETSIGDEF: i16 = 4;
const SETSIGMASK: i16 = 8;
const SETSCHEDPARAM: i16 = 16;
const SETSCHEDULER: i16 = 32;
const SETSID: i16 = 64;
const FLAGS: i16 = 127;
const EINVAL: i32 = 22;
const EBADF: i32 = 9;
const ENOMEM: i32 = 12;
const ENOENT: i32 = 2;
const EACCES: i32 = 13;
const ENOTDIR: i32 = 20;
const ENAMETOOLONG: i32 = 36;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Attr {
    flags: i16,
    pgroup: i32,
    sigdefault: u64,
    sigmask: u64,
    schedparam: i32,
    schedpolicy: i32,
}

#[repr(C)]
pub struct FileActions {
    head: *mut Action,
    tail: *mut Action,
}

struct Action {
    next: *mut Action,
    kind: Kind,
    fd: i32,
    other: i32,
    mode: u32,
    path: *mut u8,
}

#[derive(Clone, Copy)]
enum Kind {
    Close,
    Dup,
    Open,
    Chdir,
    Fchdir,
}

#[no_mangle]
pub unsafe extern "C" fn posix_spawnattr_init(attr: *mut Attr) -> i32 {
    attr.write(Attr::default());
    0
}

#[no_mangle]
pub unsafe extern "C" fn posix_spawnattr_destroy(attr: *mut Attr) -> i32 {
    attr.write(Attr::default());
    0
}

macro_rules! attribute {
    ($get:ident, $set:ident, $field:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $get(attr: *const Attr, value: *mut $ty) -> i32 {
            *value = (*attr).$field;
            0
        }
        #[no_mangle]
        pub unsafe extern "C" fn $set(attr: *mut Attr, value: $ty) -> i32 {
            (*attr).$field = value;
            0
        }
    };
}
attribute!(
    posix_spawnattr_getpgroup,
    posix_spawnattr_setpgroup,
    pgroup,
    i32
);

#[no_mangle]
pub unsafe extern "C" fn posix_spawnattr_getflags(attr: *const Attr, value: *mut i16) -> i32 {
    *value = (*attr).flags;
    0
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawnattr_setflags(attr: *mut Attr, value: i16) -> i32 {
    if value & !FLAGS != 0 || value & (SETSID | SETPGROUP) == (SETSID | SETPGROUP) {
        return EINVAL;
    }
    (*attr).flags = value;
    0
}

macro_rules! attribute_ptr {
    ($get:ident, $set:ident, $field:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $get(attr: *const Attr, value: *mut $ty) -> i32 {
            *value = (*attr).$field;
            0
        }
        #[no_mangle]
        pub unsafe extern "C" fn $set(attr: *mut Attr, value: *const $ty) -> i32 {
            (*attr).$field = *value;
            0
        }
    };
}
attribute_ptr!(
    posix_spawnattr_getsigdefault,
    posix_spawnattr_setsigdefault,
    sigdefault,
    u64
);
attribute_ptr!(
    posix_spawnattr_getsigmask,
    posix_spawnattr_setsigmask,
    sigmask,
    u64
);
attribute_ptr!(
    posix_spawnattr_getschedparam,
    posix_spawnattr_setschedparam,
    schedparam,
    i32
);

#[no_mangle]
pub unsafe extern "C" fn posix_spawnattr_getschedpolicy(attr: *const Attr, value: *mut i32) -> i32 {
    *value = (*attr).schedpolicy;
    0
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawnattr_setschedpolicy(attr: *mut Attr, value: i32) -> i32 {
    if !(0..=2).contains(&value) {
        return EINVAL;
    }
    (*attr).schedpolicy = value;
    0
}

#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_init(actions: *mut FileActions) -> i32 {
    actions.write(FileActions {
        head: null_mut(),
        tail: null_mut(),
    });
    0
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_destroy(actions: *mut FileActions) -> i32 {
    let mut node = (*actions).head;
    while !node.is_null() {
        let next = (*node).next;
        crate::free((*node).path);
        crate::free(node.cast());
        node = next;
    }
    posix_spawn_file_actions_init(actions)
}

unsafe fn c_bytes<'a>(path: *const u8) -> &'a [u8] {
    let mut len = 0;
    while *path.add(len) != 0 {
        len += 1;
    }
    core::slice::from_raw_parts(path, len)
}

unsafe fn add(
    actions: *mut FileActions,
    kind: Kind,
    fd: i32,
    other: i32,
    path: *const u8,
    mode: u32,
) -> i32 {
    let limit = crate::conf::system_value(4); // _SC_OPEN_MAX
    let valid_fd = |fd: i32| fd >= 0 && (limit < 0 || (fd as i64) < limit);
    if !matches!(kind, Kind::Chdir) && !valid_fd(fd)
        || matches!(kind, Kind::Dup) && !valid_fd(other)
    {
        return EBADF;
    }
    let copy = if path.is_null() {
        null_mut()
    } else {
        let bytes = c_bytes(path);
        let copy = crate::malloc(bytes.len() + 1);
        if copy.is_null() {
            return ENOMEM;
        }
        core::ptr::copy_nonoverlapping(path, copy, bytes.len() + 1);
        copy
    };
    let node = crate::malloc(core::mem::size_of::<Action>()).cast::<Action>();
    if node.is_null() {
        crate::free(copy);
        return ENOMEM;
    }
    node.write(Action {
        next: null_mut(),
        kind,
        fd,
        other,
        mode,
        path: copy,
    });
    if (*actions).tail.is_null() {
        (*actions).head = node;
    } else {
        (*(*actions).tail).next = node;
    }
    (*actions).tail = node;
    0
}

#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_addclose(a: *mut FileActions, fd: i32) -> i32 {
    add(a, Kind::Close, fd, 0, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_adddup2(
    a: *mut FileActions,
    fd: i32,
    to: i32,
) -> i32 {
    add(a, Kind::Dup, fd, to, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_addopen(
    a: *mut FileActions,
    fd: i32,
    path: *const u8,
    flags: i32,
    mode: u32,
) -> i32 {
    add(a, Kind::Open, fd, flags, path, mode)
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_addchdir(
    a: *mut FileActions,
    path: *const u8,
) -> i32 {
    add(a, Kind::Chdir, 0, 0, path, 0)
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_addfchdir(a: *mut FileActions, fd: i32) -> i32 {
    add(a, Kind::Fchdir, fd, 0, null(), 0)
}

unsafe fn call(number: u64, args: [u64; 6]) -> Result<i64, i32> {
    let r = raw::syscall6(number, args[0], args[1], args[2], args[3], args[4], args[5]) as i64;
    if r < 0 {
        Err(-r as i32)
    } else {
        Ok(r)
    }
}

unsafe fn duplicate(fd: i32, to: i32) -> Result<(), i32> {
    if fd == to {
        // A spawn dup2 action on the same descriptor still clears FD_CLOEXEC.
        let flags = call(nr::FCNTL, [fd as u64, 1, 0, 0, 0, 0])?;
        call(nr::FCNTL, [fd as u64, 2, (flags & !1) as u64, 0, 0, 0])?;
    } else {
        call(nr::DUP3, [fd as u64, to as u64, 0, 0, 0, 0])?;
    }
    Ok(())
}

unsafe fn close(fd: i32) -> Result<(), i32> {
    match call(nr::CLOSE, [fd as u64, 0, 0, 0, 0, 0]) {
        Ok(_) | Err(EBADF) => Ok(()),
        Err(e) => Err(e),
    }
}

unsafe fn file_actions(actions: *const FileActions) -> Result<(), i32> {
    if actions.is_null() {
        return Ok(());
    }
    let mut node = (*actions).head;
    while !node.is_null() {
        let a = &*node;
        match a.kind {
            Kind::Close => close(a.fd)?,
            Kind::Dup => duplicate(a.fd, a.other)?,
            Kind::Open => {
                close(a.fd)?;
                let opened = call(
                    nr::OPENAT,
                    [
                        (-100i64) as u64,
                        a.path as u64,
                        a.other as u64,
                        a.mode as u64,
                        0,
                        0,
                    ],
                )? as i32;
                if opened != a.fd {
                    let result = duplicate(opened, a.fd);
                    close(opened)?;
                    result?;
                }
            }
            Kind::Chdir => {
                call(nr::CHDIR, [a.path as u64, 0, 0, 0, 0, 0])?;
            }
            Kind::Fchdir => {
                call(nr::FCHDIR, [a.fd as u64, 0, 0, 0, 0, 0])?;
            }
        }
        node = a.next;
    }
    Ok(())
}

unsafe fn attributes(attr: &Attr) -> Result<(), i32> {
    if attr.flags & SETSID != 0 {
        call(nr::SETSID, [0; 6])?;
    }
    if attr.flags & SETPGROUP != 0 {
        call(nr::SETPGID, [0, attr.pgroup as u64, 0, 0, 0, 0])?;
    }
    if attr.flags & SETSCHEDULER != 0 {
        call(
            nr::SCHED_SETSCHEDULER,
            [
                0,
                attr.schedpolicy as u64,
                &attr.schedparam as *const i32 as u64,
                0,
                0,
                0,
            ],
        )?;
    } else if attr.flags & SETSCHEDPARAM != 0 {
        call(
            nr::SCHED_SETPARAM,
            [0, &attr.schedparam as *const i32 as u64, 0, 0, 0, 0],
        )?;
    }
    if attr.flags & RESETIDS != 0 {
        // Change only the effective IDs. exec establishes the new saved IDs.
        #[cfg(target_arch = "aarch64")]
        let (getuid, getgid, setreuid, setregid) = (174, 176, 145, 143);
        #[cfg(target_arch = "x86_64")]
        let (getuid, getgid, setreuid, setregid) = (102, 104, 113, 114);
        let uid = call(getuid, [0; 6])?;
        let gid = call(getgid, [0; 6])?;
        call(setregid, [u64::MAX, gid as u64, 0, 0, 0, 0])?;
        call(setreuid, [u64::MAX, uid as u64, 0, 0, 0, 0])?;
    }
    // Blocked signals cannot run inherited application handlers between fork
    // and exec. Reset caught dispositions before restoring the child mask.
    for sig in 1..=64 {
        if sig == 9 || sig == 19 {
            continue;
        }
        // Kernel sigaction layout: handler, flags, restorer, mask.
        let mut action = [0u64; 4];
        call(nr::SIGACTION, [sig, 0, action.as_mut_ptr() as u64, 8, 0, 0])?;
        if action[0] > 1 || attr.flags & SETSIGDEF != 0 && attr.sigdefault & (1 << (sig - 1)) != 0 {
            action = [0; 4];
            call(nr::SIGACTION, [sig, action.as_ptr() as u64, 0, 8, 0, 0])?;
        }
    }
    Ok(())
}

unsafe fn execute(
    file: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
    path: Option<&[u8]>,
) -> Result<(), i32> {
    let name = c_bytes(file);
    if name.is_empty() {
        return Err(ENOENT);
    }
    if path.is_none() || name.contains(&b'/') {
        call(nr::EXEC, [file as u64, argv as u64, envp as u64, 0, 0, 0])?;
        return Ok(());
    }
    let mut buffer = [0u8; 4096];
    let mut error = ENOENT;
    for dir in path.unwrap().split(|&b| b == b':') {
        let dir = if dir.is_empty() { &b"."[..] } else { dir };
        let len = dir.len() + 1 + name.len();
        if len >= buffer.len() {
            if error != EACCES {
                error = ENAMETOOLONG;
            }
            continue;
        }
        buffer[..dir.len()].copy_from_slice(dir);
        buffer[dir.len()] = b'/';
        buffer[dir.len() + 1..len].copy_from_slice(name);
        buffer[len] = 0;
        match call(
            nr::EXEC,
            [buffer.as_ptr() as u64, argv as u64, envp as u64, 0, 0, 0],
        ) {
            Ok(_) => return Ok(()),
            Err(EACCES) => error = EACCES,
            Err(ENOENT | ENOTDIR) => {}
            Err(e) => return Err(e), // Including ENOEXEC: spawn never runs a shell fallback.
        }
    }
    Err(error)
}

unsafe fn spawn(
    pid: *mut i32,
    file: *const u8,
    actions: *const FileActions,
    attr: *const Attr,
    argv: *const *const u8,
    envp: *const *const u8,
    search: bool,
) -> i32 {
    let attr = if attr.is_null() {
        Attr::default()
    } else {
        *attr
    };
    let path = if search {
        let p = crate::env::getenv(b"PATH\0".as_ptr());
        Some(if p.is_null() {
            &b"/usr/local/bin:/bin:/sbin"[..]
        } else {
            c_bytes(p)
        })
    } else {
        None
    };
    let all = u64::MAX;
    let mut old = 0u64;
    if let Err(e) = call(
        nr::SIGPROCMASK,
        [
            2,
            &all as *const u64 as u64,
            &mut old as *mut u64 as u64,
            8,
            0,
            0,
        ],
    ) {
        return e;
    }
    #[cfg(target_arch = "aarch64")]
    let child = call(nr::CLONE, [17, 0, 0, 0, 0, 0]);
    #[cfg(target_arch = "x86_64")]
    let child = call(nr::FORK, [0; 6]);
    if child == Ok(0) {
        let mask = if attr.flags & SETSIGMASK != 0 {
            attr.sigmask
        } else {
            old
        };
        let result = attributes(&attr)
            .and_then(|()| file_actions(actions))
            .and_then(|()| {
                call(nr::SIGPROCMASK, [2, &mask as *const u64 as u64, 0, 8, 0, 0])?;
                execute(file, argv, envp, path)
            });
        // A successful exec cannot return, even if the new image exits 127.
        let _ = result;
        libbreenix::process::exit(127);
    }
    let restored = call(nr::SIGPROCMASK, [2, &old as *const u64 as u64, 0, 8, 0, 0]);
    match child {
        Err(e) => e,
        Ok(child) => {
            if !pid.is_null() {
                *pid = child as i32;
            }
            // The kernel accepted this same mask before fork; restoration
            // cannot fail for a valid stack address and sigset size.
            debug_assert!(restored.is_ok());
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn posix_spawn(
    pid: *mut i32,
    path: *const u8,
    actions: *const FileActions,
    attr: *const Attr,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    spawn(pid, path, actions, attr, argv, envp, false)
}
#[no_mangle]
pub unsafe extern "C" fn posix_spawnp(
    pid: *mut i32,
    file: *const u8,
    actions: *const FileActions,
    attr: *const Attr,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    spawn(pid, file, actions, attr, argv, envp, true)
}
