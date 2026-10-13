//! Credential snapshots and the single POSIX permission-class selection.

use super::ext2::Ext2Inode;
use alloc::sync::Arc;
use alloc::vec::Vec;

#[derive(Clone)]
pub struct Credentials {
    pub euid: u32,
    pub egid: u32,
    pub umask: u32,
    groups: Option<Arc<Vec<u32>>>,
}

impl Credentials {
    pub fn current(real: bool) -> Self {
        let mut cred = Self {
            euid: 0,
            egid: 0,
            umask: 0,
            groups: None,
        };
        if let Some(tid) = crate::task::scheduler::current_thread_id() {
            let guard = crate::process::manager();
            if let Some(manager) = guard.as_ref() {
                if let Some((_, process)) = manager.find_process_by_thread(tid) {
                    let ids = &process.cred;
                    cred.euid = if real { ids.uid } else { ids.euid };
                    cred.egid = if real { ids.gid } else { ids.egid };
                    cred.umask = process.umask;
                    cred.groups = Some(ids.groups.clone());
                }
            }
        }
        cred
    }

    pub fn in_group(&self, gid: u32) -> bool {
        self.egid == gid
            || self
                .groups
                .as_ref()
                .is_some_and(|groups| groups.contains(&gid))
    }

    pub fn permits(&self, inode: &Ext2Inode, wanted: u32) -> bool {
        self.permits_mode(inode.uid(), inode.gid(), inode.i_mode as u32, wanted)
    }

    pub fn permits_mode(&self, uid: u32, gid: u32, mode: u32, wanted: u32) -> bool {
        if self.euid == 0 {
            return wanted & 1 == 0 || mode & 0o170000 == 0o040000 || mode & 0o111 != 0;
        }
        let granted = if self.euid == uid {
            (mode >> 6) & 7
        } else if self.in_group(gid) {
            (mode >> 3) & 7
        } else {
            mode & 7
        };
        granted & wanted == wanted
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
            groups: Some(Arc::new(groups.to_vec())),
        }
    }

    #[test]
    fn permission_classes_do_not_fall_through() {
        let owner = credentials(10, 20, &[30]);
        assert!(!owner.permits_mode(10, 30, 0o004, 4));
        assert!(owner.permits_mode(10, 30, 0o400, 4));
        let member = credentials(11, 20, &[30]);
        assert!(!member.permits_mode(10, 30, 0o004, 4));
        assert!(member.permits_mode(10, 30, 0o040, 4));
        assert!(member.permits_mode(10, 20, 0o020, 2));
        assert!(member.permits_mode(10, 40, 0o006, 6));
        assert!(!member.permits_mode(10, 40, 0o004, 6));
    }

    #[test]
    fn root_execute_policy_preserves_directories() {
        let root = credentials(0, 0, &[]);
        assert!(root.permits_mode(10, 20, 0o010000, 6));
        assert!(!root.permits_mode(10, 20, 0o100000, 1));
        assert!(root.permits_mode(10, 20, 0o100001, 1));
        assert!(root.permits_mode(10, 20, 0o040000, 1));
    }

    #[test]
    fn inode_and_mode_checks_agree() {
        let cred = credentials(10, 20, &[30]);
        let mut inode = Ext2Inode::new_regular_file(0, 0, 0);
        inode.set_owner(11, 30);
        for mode in [0o000, 0o007, 0o070, 0o700, 0o754] {
            inode.i_mode = 0o100000 | mode;
            for wanted in 0..8 {
                assert_eq!(
                    cred.permits(&inode, wanted),
                    cred.permits_mode(11, 30, inode.i_mode as u32, wanted)
                );
            }
        }
    }
}
