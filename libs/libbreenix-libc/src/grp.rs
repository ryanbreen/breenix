//! The group database (POSIX.1-2024 <grp.h>) over /etc/group, whose lines are
//! `name:password:gid:member,member,...`. Malformed lines are skipped. The `_r`
//! functions and the static-storage ones behave as in pwd.rs.

use crate::pwd::{Database, Packer};
use core::ptr::{addr_of_mut, null_mut};

const ERANGE: i32 = 34;

#[repr(C)]
pub struct Group {
    gr_name: *mut u8,
    gr_passwd: *mut u8,
    gr_gid: u32,
    gr_mem: *mut *mut u8,
}

impl Group {
    const fn empty() -> Self {
        Group { gr_name: null_mut(), gr_passwd: null_mut(), gr_gid: 0, gr_mem: null_mut() }
    }
}

fn parse(line: &[u8]) -> Option<([&[u8]; 4], u32)> {
    let g = libbreenix::accounts::parse::Group::parse(line)?;
    Some((g.fields, g.gid))
}

fn members(list: &[u8]) -> impl Iterator<Item = &[u8]> {
    list.split(|&b| b == b',').filter(|m| !m.is_empty())
}

/// Fill `grp` from a parsed line. Err(ERANGE) when it does not fit in `buf`.
unsafe fn fill(f: &[&[u8]; 4], gid: u32, grp: *mut Group, buf: *mut u8, len: usize) -> Result<(), i32> {
    let mut packer = Packer::new(buf, len);
    let count = members(f[3]).count();
    let list = packer.pointers(count + 1).ok_or(ERANGE)?;
    for (i, member) in members(f[3]).enumerate() {
        *list.add(i) = packer.string(member).ok_or(ERANGE)?;
    }
    *list.add(count) = null_mut();
    let name = packer.string(f[0]).ok_or(ERANGE)?;
    let passwd = packer.string(f[1]).ok_or(ERANGE)?;
    grp.write(Group { gr_name: name, gr_passwd: passwd, gr_gid: gid, gr_mem: list });
    Ok(())
}

unsafe fn lookup(
    want: &dyn Fn(&[u8], u32) -> bool,
    grp: *mut Group,
    buf: *mut u8,
    len: usize,
    result: *mut *mut Group,
) -> i32 {
    *result = null_mut();
    let mut db = match Database::open(b"/etc/group\0") {
        Ok(db) => db,
        Err(e) => return e,
    };
    let mut status = 0;
    while let Some(line) = db.next_line() {
        if let Some((f, gid)) = parse(line) {
            if want(f[0], gid) {
                match fill(&f, gid, grp, buf, len) {
                    Ok(()) => *result = grp,
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
pub unsafe extern "C" fn getgrnam_r(
    name: *const u8, grp: *mut Group, buf: *mut u8, len: usize, result: *mut *mut Group,
) -> i32 {
    let name = c_bytes(name);
    lookup(&|n, _| n == name, grp, buf, len, result)
}

#[no_mangle]
pub unsafe extern "C" fn getgrgid_r(
    gid: u32, grp: *mut Group, buf: *mut u8, len: usize, result: *mut *mut Group,
) -> i32 {
    lookup(&|_, g| g == gid, grp, buf, len, result)
}

static mut ENTRY: Group = Group::empty();
static mut ENTRY_BUF: [u8; 4096] = [0; 4096];
static mut ENUMERATION: Database = Database::empty();

unsafe fn static_result(status: i32, result: *mut Group) -> *mut Group {
    if status != 0 {
        crate::pthread::set_errno(status);
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn getgrnam(name: *const u8) -> *mut Group {
    let mut result = null_mut();
    let status = getgrnam_r(name, addr_of_mut!(ENTRY), addr_of_mut!(ENTRY_BUF) as *mut u8, 4096, &mut result);
    static_result(status, result)
}

#[no_mangle]
pub unsafe extern "C" fn getgrgid(gid: u32) -> *mut Group {
    let mut result = null_mut();
    let status = getgrgid_r(gid, addr_of_mut!(ENTRY), addr_of_mut!(ENTRY_BUF) as *mut u8, 4096, &mut result);
    static_result(status, result)
}

#[no_mangle]
pub unsafe extern "C" fn setgrent() {
    let db = &mut *addr_of_mut!(ENUMERATION);
    if db.is_open() {
        db.rewind();
    }
}

#[no_mangle]
pub unsafe extern "C" fn endgrent() {
    (*addr_of_mut!(ENUMERATION)).close();
}

#[no_mangle]
pub unsafe extern "C" fn getgrent() -> *mut Group {
    let db = &mut *addr_of_mut!(ENUMERATION);
    if !db.is_open() {
        match Database::open(b"/etc/group\0") {
            Ok(opened) => *db = opened,
            Err(e) => {
                crate::pthread::set_errno(e);
                return null_mut();
            }
        }
    }
    while let Some(line) = db.next_line() {
        if let Some((f, gid)) = parse(line) {
            return match fill(&f, gid, addr_of_mut!(ENTRY), addr_of_mut!(ENTRY_BUF) as *mut u8, 4096) {
                Ok(()) => addr_of_mut!(ENTRY),
                Err(e) => static_result(e, null_mut()),
            };
        }
    }
    null_mut()
}

/// getgrouplist: `group` and every group whose member list names `user`, stored into
/// `groups` up to `*ngroups` entries. Returns the count, or -1 (with `*ngroups` set to
/// the count needed) when they do not all fit.
#[no_mangle]
pub unsafe extern "C" fn getgrouplist(user: *const u8, group: u32, groups: *mut u32, ngroups: *mut i32) -> i32 {
    let user = c_bytes(user);
    let room = (*ngroups).max(0) as usize;
    let mut count = 0usize;
    let add = |gid: u32, count: &mut usize| {
        if *count < room {
            *groups.add(*count) = gid;
        }
        *count += 1;
    };
    add(group, &mut count);
    let mut db = match Database::open(b"/etc/group\0") {
        Ok(db) => db,
        Err(e) => {
            crate::pthread::set_errno(e);
            return -1;
        }
    };
    {
        while let Some(line) = db.next_line() {
            if let Some((f, gid)) = parse(line) {
                if gid != group && members(f[3]).any(|m| m == user) {
                    add(gid, &mut count);
                }
            }
        }
        db.close();
    }
    *ngroups = count as i32;
    if count > room {
        crate::pthread::set_errno(ERANGE);
        -1
    } else { count as i32 }
}

/// initgroups: set the supplementary groups to those getgrouplist finds for `user`.
#[no_mangle]
pub unsafe extern "C" fn initgroups(user: *const u8, group: u32) -> i32 {
    // Match this implementation's include/limits.h NGROUPS_MAX. Grow outside
    // the stack and never replace credentials after an incomplete database read.
    const MAX_GROUPS: usize = 65536;
    let mut room = 64usize;
    loop {
        let list = crate::malloc(room * core::mem::size_of::<u32>()) as *mut u32;
        if list.is_null() {
            crate::pthread::set_errno(12); // ENOMEM in Breenix's errno ABI.
            return -1;
        }
        let mut n = room as i32;
        let result = getgrouplist(user, group, list, &mut n);
        if result >= 0 {
            let status = crate::setgroups(n as usize, list);
            let error = crate::pthread::errno();
            crate::free(list as *mut u8);
            crate::pthread::set_errno(error);
            return status;
        }
        let error = crate::pthread::errno();
        crate::free(list as *mut u8);
        if error != ERANGE || n as usize <= room || n as usize > MAX_GROUPS {
            crate::pthread::set_errno(error);
            return -1;
        }
        room = n as usize;
    }
}
