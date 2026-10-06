//! Credential snapshots and the single POSIX permission-class selection.

use super::ext2::Ext2Inode;
use alloc::sync::Arc;
use alloc::vec::Vec;

#[derive(Clone)]
pub(crate) struct Credentials {
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
        if self.euid == 0 {
            // Linux policy: root needs an execute bit on any non-directory inode.
            return wanted & 1 == 0 || inode.is_dir() || inode.permissions() & 0o111 != 0;
        }
        let mode = inode.permissions() as u32;
        let granted = if self.euid == inode.uid() {
            (mode >> 6) & 7
        } else if self.in_group(inode.gid()) {
            (mode >> 3) & 7
        } else {
            mode & 7
        };
        granted & wanted == wanted
    }
}
