//! A process's user and group identity, and the POSIX rules that change it.
//!
//! One `ProcessCredentials` per process row holds the real, effective and
//! saved set-user-ID and set-group-ID and the supplementary group list. Fork
//! copies it, exec transforms it (`exec`), and the set-ID calls change it only
//! through the methods below. A process whose effective user ID is 0 holds every
//! privilege these rules name (Linux's CAP_SETUID, CAP_SETGID and CAP_SYS_NICE).
//! There is no separate filesystem user ID: permission checks use the
//! effective IDs, which is what a filesystem ID follows unless setfsuid moves it.

use crate::syscall::errno::{EINVAL, EPERM};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The ID argument that leaves an ID unchanged in setreuid and setregid.
pub const KEEP_ID: u32 = u32::MAX;

#[derive(Clone)]
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
}

/// What an executable file confers at exec: its owner when its set-user-ID
/// bit is set and its group when its set-group-ID bit is set.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ExecIdentity {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

impl ExecIdentity {
    pub fn of(inode: &crate::fs::ext2::Ext2Inode) -> Self {
        let mode = inode.permissions();
        Self {
            uid: (mode & 0o4000 != 0).then(|| inode.uid()),
            gid: (mode & 0o2000 != 0).then(|| inode.gid()),
        }
    }
}

impl ProcessCredentials {
    /// Every ID 0 and no supplementary groups: the first process's identity.
    pub fn root() -> Self {
        Self { uid: 0, euid: 0, suid: 0, gid: 0, egid: 0, sgid: 0, groups: Arc::new(Vec::new()) }
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

    /// exec: the image's set-ID bits replace the effective IDs, then the saved
    /// IDs take the effective ones, whether or not any bit was set.
    pub fn exec(&mut self, image: ExecIdentity) {
        if let Some(uid) = image.uid {
            self.euid = uid;
        }
        if let Some(gid) = image.gid {
            self.egid = gid;
        }
        self.suid = self.euid;
        self.sgid = self.egid;
    }

    /// Whether this process may change `target`'s scheduling priority:
    /// privileged, or its effective user ID is the target's real or effective one.
    pub fn may_renice(&self, target: &ProcessCredentials) -> bool {
        self.privileged() || self.euid == target.uid || self.euid == target.euid
    }
}

/// The setreuid/setregid rule on `[real, effective, saved]`, as Linux applies
/// it within what POSIX allows. Unprivileged, the real ID may become the
/// real or effective one, and the effective ID the real, effective or saved one.
/// The saved ID takes the new effective ID whenever the real ID is set, or the
/// effective ID is set to something other than the old real ID.
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
