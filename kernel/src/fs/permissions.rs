//! Credential snapshots and the one POSIX access check.
//!
//! A permission decision on a file goes through `Credentials::may`, which
//! reads only the neutral attributes `{mode, uid, gid}` (`fs::attrs`), whatever
//! filesystem or virtual node they come from.

pub use super::attrs::{InodeAttrs, MountFlags};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The error a check gets for a user thread with no process: it has no
/// identity, and is refused rather than checked as anyone.
pub const NO_PROCESS: u64 = 3; // ESRCH

#[derive(Clone)]
pub struct Credentials {
    pub euid: u32,
    pub egid: u32,
    pub umask: u32,
    groups: Arc<Vec<u32>>,
}

impl Credentials {
    /// The identity kernel threads act with: user and group 0, no mask.
    pub fn kernel() -> Self {
        Self { euid: 0, egid: 0, umask: 0, groups: Arc::new(Vec::new()) }
    }

    /// The calling thread's effective (or, with `real`, real) user and group,
    /// its supplementary groups and its file creation mask. A kernel thread,
    /// or boot code running before its thread is registered, gets `kernel()`.
    /// A user thread whose process cannot be found fails with `NO_PROCESS`, so it
    /// is not checked as root.
    pub fn current(real: bool) -> Result<Self, u64> {
        let Some(tid) = crate::task::scheduler::current_thread_id() else {
            return Ok(Self::kernel());
        };
        // A suite boot's forcing point stands for the lookup failing.
        if crate::test_control::take(crate::test_control::CRED_ORPHAN, tid).is_none() {
            let guard = crate::process::manager();
            if let Some((_, process)) = guard.as_ref().and_then(|manager| manager.find_process_by_thread(tid)) {
                let ids = &process.cred;
                return Ok(Self {
                    euid: if real { ids.uid } else { ids.euid },
                    egid: if real { ids.gid } else { ids.egid },
                    umask: process.cwd.umask(),
                    groups: ids.groups.clone(),
                });
            }
        }
        if crate::task::scheduler::thread_is_user(tid) {
            Err(NO_PROCESS)
        } else {
            Ok(Self::kernel())
        }
    }

    pub fn privileged(&self) -> bool {
        self.euid == 0
    }

    pub fn in_group(&self, gid: u32) -> bool {
        self.egid == gid || self.groups.contains(&gid)
    }

    /// Whether these credentials grant `wanted` (read 4, write 2, execute or
    /// search 1, or a sum) on a file with `attrs`. Exactly one class applies:
    /// the owner's bits for the owner, else the group's for a member of the
    /// file's group, else the others'; a class does not fall through to the
    /// next. Root is granted read and write, and execute when the file is a
    /// directory or has an execute bit in one of the three classes.
    pub fn may(&self, attrs: InodeAttrs, wanted: u32) -> bool {
        let mode = attrs.mode;
        if self.privileged() {
            return wanted & 1 == 0 || attrs.is_dir() || mode & 0o111 != 0;
        }
        let granted = if self.euid == attrs.uid {
            (mode >> 6) & 7
        } else if self.in_group(attrs.gid) {
            (mode >> 3) & 7
        } else {
            mode & 7
        };
        granted & wanted == wanted
    }

    /// The exec check on a program file with `attrs` on a mount with `flags`:
    /// EACCES when the mount is `noexec` or execute is not granted, else the
    /// identity its set-ID bits confer, which a `nosuid` mount withholds.
    /// The caller has already refused a directory with its own errno.
    pub fn may_exec(
        &self,
        attrs: InodeAttrs,
        flags: MountFlags,
    ) -> Result<crate::process::credentials::ExecIdentity, i32> {
        const EACCES: i32 = 13;
        if flags.noexec || !self.may(attrs, 1) {
            return Err(EACCES);
        }
        Ok(crate::process::credentials::ExecIdentity::of(attrs, flags.nosuid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(uid: u32, gid: u32, groups: &[u32]) -> Credentials {
        Credentials {
            euid: uid,
            egid: gid,
            umask: 0,
            groups: Arc::new(groups.to_vec()),
        }
    }

    fn attrs(uid: u32, gid: u32, mode: u32) -> InodeAttrs {
        InodeAttrs::new(mode, uid, gid)
    }

    #[test]
    fn permission_classes_do_not_fall_through() {
        let owner = credentials(10, 20, &[30]);
        assert!(!owner.may(attrs(10, 30, 0o004), 4));
        assert!(owner.may(attrs(10, 30, 0o400), 4));
        let member = credentials(11, 20, &[30]);
        assert!(!member.may(attrs(10, 30, 0o004), 4));
        assert!(member.may(attrs(10, 30, 0o040), 4));
        assert!(member.may(attrs(10, 20, 0o020), 2));
        assert!(member.may(attrs(10, 40, 0o006), 6));
        assert!(!member.may(attrs(10, 40, 0o004), 6));
    }

    #[test]
    fn root_execute_policy_preserves_directories() {
        let root = Credentials::kernel();
        assert!(root.may(attrs(10, 20, 0o010000), 6));
        assert!(!root.may(attrs(10, 20, 0o100000), 1));
        assert!(root.may(attrs(10, 20, 0o100001), 1));
        assert!(root.may(attrs(10, 20, 0o040000), 1));
    }

    #[test]
    fn exec_honours_noexec_and_nosuid() {
        let user = credentials(10, 20, &[]);
        let program = attrs(30, 40, 0o106755);
        let found = user.may_exec(program, MountFlags::default()).unwrap();
        assert_eq!((found.uid, found.gid), (Some(30), Some(40)));
        let nosuid = MountFlags { nosuid: true, ..MountFlags::default() };
        assert!(!user.may_exec(program, nosuid).unwrap().any());
        let noexec = MountFlags { noexec: true, ..MountFlags::default() };
        assert_eq!(user.may_exec(program, noexec).err(), Some(13));
        assert_eq!(Credentials::kernel().may_exec(program, noexec).err(), Some(13));
        let no_group_execute = attrs(30, 40, 0o102745);
        assert_eq!(user.may_exec(no_group_execute, MountFlags::default()).unwrap().gid, None);
    }
}
