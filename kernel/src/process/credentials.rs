//! A process's user and group identity, and the POSIX rules that change it.
//!
//! One `ProcessCredentials` per process row holds the real, effective and
//! saved set-user-ID and set-group-ID and the supplementary group list. Fork
//! copies it, exec transforms it (`after_exec`), and the set-ID calls change it
//! only through the methods below; the system calls commit each change to the
//! process's threads at once. A process whose effective user ID is 0 holds
//! the privileges these rules name: there is no finer split of privilege (the
//! POSIX.1e capability draft was withdrawn). There is no separate filesystem
//! user ID: permission checks use the effective IDs.

use crate::syscall::errno::{EINVAL, EPERM};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The ID argument that leaves an ID unchanged in setreuid, setregid,
/// setresuid and setresgid.
pub const KEEP_ID: u32 = u32::MAX;

/// The most supplementary groups a process may hold.
pub const NGROUPS_MAX: usize = 65536;

#[derive(Clone, PartialEq, Eq)]
pub struct ProcessCredentials {
    /// Real user ID.
    pub uid: u32,
    /// Effective user ID.
    pub euid: u32,
    /// Saved set-user-ID.
    pub suid: u32,
    /// Real group ID.
    pub gid: u32,
    /// Effective group ID.
    pub egid: u32,
    /// Saved set-group-ID.
    pub sgid: u32,
    /// Supplementary group membership, shared until setgroups replaces it.
    pub groups: Arc<Vec<u32>>,
    /// The last exec changed the effective user or group ID: the program
    /// runs with an identity its invoker did not have, so its invoker may not
    /// inspect it (`privilege::may_inspect`).
    pub gained_at_exec: bool,
}

/// What an executable file confers at exec: its owner when its set-user-ID
/// bit is set, and its group when its set-group-ID bit and its group execute
/// bit are both set. A set-group-ID bit without group execute confers no group:
/// System V used that combination to mark a file for mandatory locking, and
/// a program no group member may execute has no group identity to run with.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ExecIdentity {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

impl ExecIdentity {
    /// The identity a file with these attributes confers; an empty one when
    /// it is on a filesystem mounted `nosuid`.
    pub fn of(attrs: crate::fs::attrs::InodeAttrs, nosuid: bool) -> Self {
        if nosuid {
            return Self::default();
        }
        let mode = attrs.mode;
        Self {
            uid: (mode & 0o4000 != 0).then_some(attrs.uid),
            gid: (mode & 0o2010 == 0o2010).then_some(attrs.gid),
        }
    }

    /// Whether the file confers any identity.
    pub fn any(&self) -> bool {
        self.uid.is_some() || self.gid.is_some()
    }
}

impl ProcessCredentials {
    /// Every ID 0 and no supplementary groups: the first process's identity.
    pub fn root() -> Self {
        Self {
            uid: 0,
            euid: 0,
            suid: 0,
            gid: 0,
            egid: 0,
            sgid: 0,
            groups: Arc::new(Vec::new()),
            gained_at_exec: false,
        }
    }

    pub fn privileged(&self) -> bool {
        self.euid == 0
    }

    /// setuid: privileged, set all three user IDs; otherwise only the
    /// effective one, and only to the real or saved user ID.
    pub fn setuid(&mut self, uid: u32) -> Result<(), u64> {
        if uid == KEEP_ID {
            return Err(EINVAL as u64);
        }
        if self.privileged() {
            (self.uid, self.euid, self.suid) = (uid, uid, uid);
        } else if uid == self.uid || uid == self.suid {
            self.euid = uid;
        } else {
            return Err(EPERM as u64);
        }
        Ok(())
    }

    /// setgid: the same rule for the group IDs, with privilege still taken
    /// from the effective user ID.
    pub fn setgid(&mut self, gid: u32) -> Result<(), u64> {
        if gid == KEEP_ID {
            return Err(EINVAL as u64);
        }
        if self.privileged() {
            (self.gid, self.egid, self.sgid) = (gid, gid, gid);
        } else if gid == self.gid || gid == self.sgid {
            self.egid = gid;
        } else {
            return Err(EPERM as u64);
        }
        Ok(())
    }

    /// setresuid: see `set_real_effective_saved`.
    pub fn setresuid(&mut self, real: u32, effective: u32, saved: u32) -> Result<(), u64> {
        let privileged = self.privileged();
        let ids = set_real_effective_saved([self.uid, self.euid, self.suid], [real, effective, saved], privileged)?;
        [self.uid, self.euid, self.suid] = ids;
        Ok(())
    }

    /// setresgid: see `set_real_effective_saved`, with privilege still taken
    /// from the effective user ID.
    pub fn setresgid(&mut self, real: u32, effective: u32, saved: u32) -> Result<(), u64> {
        let privileged = self.privileged();
        let ids = set_real_effective_saved([self.gid, self.egid, self.sgid], [real, effective, saved], privileged)?;
        [self.gid, self.egid, self.sgid] = ids;
        Ok(())
    }

    /// setgroups: only a privileged process may replace its supplementary groups.
    pub fn setgroups(&mut self, groups: Arc<Vec<u32>>) -> Result<(), u64> {
        if !self.privileged() {
            return Err(EPERM as u64);
        }
        if groups.len() > NGROUPS_MAX {
            return Err(EINVAL as u64);
        }
        self.groups = groups;
        Ok(())
    }

    /// setreuid: see `set_real_effective`.
    pub fn setreuid(&mut self, real: u32, effective: u32) -> Result<(), u64> {
        let privileged = self.privileged();
        let ids = set_real_effective([self.uid, self.euid, self.suid], real, effective, privileged)?;
        [self.uid, self.euid, self.suid] = ids;
        Ok(())
    }

    /// setregid: see `set_real_effective`.
    pub fn setregid(&mut self, real: u32, effective: u32) -> Result<(), u64> {
        let privileged = self.privileged();
        let ids = set_real_effective([self.gid, self.egid, self.sgid], real, effective, privileged)?;
        [self.gid, self.egid, self.sgid] = ids;
        Ok(())
    }

    /// The credentials an exec of `image` leaves: its set-ID bits replace the
    /// effective IDs, then the saved IDs take the effective ones, whether or
    /// not any bit was set.
    ///
    /// `shared_fs` says another process shares the caller's working
    /// directory, root directory and umask (CLONE_FS) and outlives the exec.
    /// Unless the caller is already privileged, such an exec gains no identity:
    /// the effective IDs fall back to the real ones. The hazard is the sharer,
    /// which keeps the caller's identity: it could chdir or chroot the
    /// privileged program out from under it, or change the mask its files are
    /// created with, while the program runs with an identity the sharer lacks.
    pub fn after_exec(&self, image: ExecIdentity, shared_fs: bool) -> Self {
        let mut new = self.clone();
        if let Some(uid) = image.uid {
            new.euid = uid;
        }
        if let Some(gid) = image.gid {
            new.egid = gid;
        }
        let gains = new.euid != self.uid || new.egid != self.gid;
        if shared_fs && gains && !self.privileged() {
            new.euid = new.uid;
            new.egid = new.gid;
        }
        new.suid = new.euid;
        new.sgid = new.egid;
        new.gained_at_exec = new.euid != self.euid || new.egid != self.egid;
        new
    }

    /// Whether a program running with these credentials must start in secure
    /// mode (AT_SECURE): its effective identity is not its real one.
    pub fn secure(&self) -> bool {
        self.euid != self.uid || self.egid != self.gid
    }

    /// Whether this process may read `target`'s private state
    /// (`privilege::may_inspect`): root may; otherwise this process's real
    /// user and group must equal each of the target's three, and the target
    /// must not have gained an identity at exec.
    pub fn may_inspect(&self, target: &ProcessCredentials) -> bool {
        if self.privileged() {
            return true;
        }
        !target.gained_at_exec
            && [target.uid, target.euid, target.suid].iter().all(|&id| id == self.uid)
            && [target.gid, target.egid, target.sgid].iter().all(|&id| id == self.gid)
    }

    /// Whether this process may change `target`'s scheduling priority:
    /// privileged, or its effective user ID is the target's real or effective one.
    pub fn may_renice(&self, target: &ProcessCredentials) -> bool {
        self.privileged() || self.euid == target.uid || self.euid == target.euid
    }
}

/// The setresuid/setresgid rule on `[real, effective, saved]`. A privileged
/// caller may set each to any value. Unprivileged, each new ID must be one of
/// the three current ones, so a program can drop or swap its identities in one
/// call and cannot gain one it does not hold. `KEEP_ID` leaves an ID unchanged.
/// When one ID is refused, the call changes no ID.
fn set_real_effective_saved(old: [u32; 3], new: [u32; 3], privileged: bool) -> Result<[u32; 3], u64> {
    let mut ids = old;
    for (slot, id) in ids.iter_mut().zip(new) {
        if id == KEEP_ID {
            continue;
        }
        if !privileged && !old.contains(&id) {
            return Err(EPERM as u64);
        }
        *slot = id;
    }
    Ok(ids)
}

/// The setreuid/setregid rule on `[real, effective, saved]`, within what
/// POSIX allows. Unprivileged, the real ID may become the real or effective
/// one, and the effective ID the real, effective or saved one. The saved ID
/// takes the new effective ID whenever the real ID is set, or the effective
/// ID is set to something other than the old real ID, so a program that
/// swaps its real and effective IDs cannot later regain the old effective one
/// through the saved ID unless it kept it deliberately.
fn set_real_effective(old: [u32; 3], real: u32, effective: u32, privileged: bool) -> Result<[u32; 3], u64> {
    let [old_real, old_effective, old_saved] = old;
    let mut new = old;
    if real != KEEP_ID {
        if !privileged && real != old_real && real != old_effective {
            return Err(EPERM as u64);
        }
        new[0] = real;
    }
    if effective != KEEP_ID {
        if !privileged && effective != old_real && effective != old_effective && effective != old_saved {
            return Err(EPERM as u64);
        }
        new[1] = effective;
    }
    if real != KEEP_ID || (effective != KEEP_ID && effective != old_real) {
        new[2] = new[1];
    }
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(uid: u32, gid: u32) -> ProcessCredentials {
        let mut cred = ProcessCredentials::root();
        (cred.uid, cred.euid, cred.suid, cred.gid, cred.egid, cred.sgid) = (uid, uid, uid, gid, gid, gid);
        cred
    }

    #[test]
    fn setres_takes_only_current_ids_unprivileged() {
        let mut cred = user(10, 20);
        cred.euid = 11;
        cred.suid = 12;
        assert_eq!(cred.setresuid(12, 10, 11), Ok(()));
        assert_eq!((cred.uid, cred.euid, cred.suid), (12, 10, 11));
        assert_eq!(cred.setresuid(13, KEEP_ID, KEEP_ID), Err(EPERM as u64));
        assert_eq!(cred.setresuid(KEEP_ID, 0, KEEP_ID), Err(EPERM as u64));
        assert_eq!((cred.uid, cred.euid, cred.suid), (12, 10, 11));
        assert_eq!(cred.setresgid(20, 20, 21), Err(EPERM as u64));
        let mut root = ProcessCredentials::root();
        assert_eq!(root.setresgid(30, 31, 32), Ok(()));
        assert_eq!(root.setresuid(40, 41, 42), Ok(()));
        assert_eq!((root.uid, root.euid, root.suid, root.gid, root.egid, root.sgid), (40, 41, 42, 30, 31, 32));
    }

    #[test]
    fn setgroups_needs_privilege() {
        let mut cred = user(10, 20);
        assert_eq!(cred.setgroups(Arc::new(alloc::vec![1])), Err(EPERM as u64));
        let mut root = ProcessCredentials::root();
        assert_eq!(root.setgroups(Arc::new(alloc::vec![1, 2])), Ok(()));
        assert_eq!(*root.groups, [1, 2]);
    }

    #[test]
    fn inspection_needs_matching_ids_and_no_gain() {
        let me = user(5, 6);
        assert!(me.may_inspect(&user(5, 6)));
        assert!(ProcessCredentials::root().may_inspect(&user(7, 8)));
        let mut other = user(5, 6);
        other.suid = 0;
        assert!(!me.may_inspect(&other));
        let mut group = user(5, 6);
        group.egid = 9;
        assert!(!me.may_inspect(&group));
        let setuid = me.after_exec(ExecIdentity { uid: Some(7), gid: None }, false);
        assert!(setuid.gained_at_exec);
        let mut dropped = setuid.clone();
        dropped.setuid(5).unwrap();
        assert!(!me.may_inspect(&dropped));
        assert!(!me.after_exec(ExecIdentity::default(), false).gained_at_exec);
    }
}
