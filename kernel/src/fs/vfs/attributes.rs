//! Credential policy for mode, ownership and timestamp changes.
use super::{SetAttr, Timestamp};
use crate::fs::{attrs::InodeAttrs, errno::Errno, permissions::Credentials};
use crate::syscall::errno::{EACCES, EPERM};

#[derive(Clone, Copy)]
pub enum AttributeChange {
    Mode(u32),
    Owner(u32, u32),
}

/// Decide a metadata change without accessing storage. The backend compares
/// the permission snapshot under mutation exclusion before applying it.
pub fn attribute_change(
    attrs: InodeAttrs,
    change: AttributeChange,
    cred: &Credentials,
) -> Result<SetAttr, Errno> {
    if !cred.privileged() && cred.euid != attrs.uid {
        return Err(Errno(EPERM));
    }
    let mut request = SetAttr { expected: Some(attrs), ..SetAttr::default() };
    match change {
        AttributeChange::Mode(mode) => {
            let mut bits = (mode & 0o7777) as u16;
            if !cred.privileged() && !cred.in_group(attrs.gid) {
                bits &= !0o2000;
            }
            request.mode = Some(bits);
        }
        AttributeChange::Owner(uid, gid) => {
            if !cred.privileged()
                && ((uid != u32::MAX && uid != attrs.uid)
                    || (gid != u32::MAX && gid != attrs.gid && !cred.in_group(gid)))
            {
                return Err(Errno(EPERM));
            }
            request.uid = (uid != u32::MAX).then_some(uid);
            request.gid = (gid != u32::MAX).then_some(gid);
            // Regular files lose execution privileges even for root.
            // Directories retain SGID for group inheritance.
            if !attrs.is_dir() && (attrs.is_regular() || !cred.privileged()) {
                request.mode = Some((attrs.permissions() & !0o6000) as u16);
            }
        }
    }
    Ok(request)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimesAccess {
    OwnerOrWrite,
    Owner,
}

pub fn timestamp_change(
    attrs: InodeAttrs,
    atime: Option<Timestamp>,
    mtime: Option<Timestamp>,
    cred: &Credentials,
    access: TimesAccess,
) -> Result<SetAttr, Errno> {
    if !cred.privileged() && cred.euid != attrs.uid {
        match access {
            TimesAccess::Owner => return Err(Errno(EPERM)),
            TimesAccess::OwnerOrWrite if !cred.may(attrs, 2) => return Err(Errno(EACCES)),
            TimesAccess::OwnerOrWrite => {}
        }
    }
    Ok(SetAttr { expected: Some(attrs), atime, mtime, ..SetAttr::default() })
}
