//! Mknod types and parent-directory policy, without namespace or PM locks.
use crate::fs::{ext2::Ext2Inode, permissions::Credentials};

pub(crate) fn node_type(mode: u32) -> Result<u16, u64> {
    match mode & 0o170000 {
        0 | 0o100000 => Ok(0o100000),
        0o010000 | 0o020000 | 0o060000 | 0o140000 => Ok((mode & 0o170000) as u16),
        0o040000 => Err(1),
        _ => Err(22),
    }
}

pub(crate) fn check_parent_access(
    parent: &Ext2Inode,
    cred: &Credentials,
    owner: Option<u32>,
) -> Result<u32, u64> {
    if !parent.is_dir() {
        return Err(20);
    }
    if parent.i_links_count == 0 {
        return Err(2);
    }
    if !cred.permits(parent, 3) {
        return Err(13);
    }
    if let Some(owner) = owner {
        if parent.permissions() & 0o1000 != 0
            && cred.euid != 0
            && cred.euid != parent.uid()
            && cred.euid != owner
        {
            return Err(1); // Linux sticky-directory unlink: EPERM.
        }
    }
    Ok(if parent.permissions() & 0o2000 != 0 {
        parent.gid()
    } else {
        cred.egid
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn cred(uid: u32, gid: u32) -> Credentials {
        let mut cred = Credentials::current(false);
        cred.euid = uid;
        cred.egid = gid;
        cred
    }
    #[test]
    fn creation_requires_write_and_search_in_the_selected_class() {
        let mut parent = Ext2Inode::new_directory(0o777);
        parent.set_owner(10, 20);
        for mode in [0o444, 0o666, 0o555, 0o711] {
            parent.i_mode = 0o040000 | mode;
            assert_eq!(check_parent_access(&parent, &cred(11, 21), None), Err(13));
        }
        parent.i_mode = 0o040733;
        assert_eq!(check_parent_access(&parent, &cred(11, 20), None), Ok(20));
        parent.i_links_count = 0;
        assert_eq!(check_parent_access(&parent, &cred(0, 0), None), Err(2));
    }
    #[test]
    fn sticky_unlink_allows_root_parent_owner_and_target_owner() {
        let mut parent = Ext2Inode::new_directory(0o1777);
        parent.set_owner(10, 20);
        assert_eq!(
            check_parent_access(&parent, &cred(11, 21), Some(12)),
            Err(1)
        );
        for uid in [0, 10, 12] {
            assert!(check_parent_access(&parent, &cred(uid, 21), Some(12)).is_ok());
        }
        assert_eq!(check_parent_access(&parent, &cred(11, 21), None), Ok(21));
    }
    #[test]
    fn setgid_parent_supplies_its_group_for_fifo_creation() {
        let mut parent = Ext2Inode::new_directory(0o777);
        parent.i_mode = 0o042777;
        parent.set_owner(10, 30000);
        assert_eq!(check_parent_access(&parent, &cred(11, 21), None), Ok(30000));
        parent.i_mode = 0o040777;
        assert_eq!(check_parent_access(&parent, &cred(11, 21), None), Ok(21));
    }
    #[test]
    fn mknod_types_match_linux_errors() {
        for mode in [0, 0o100000, 0o010000, 0o020000, 0o060000, 0o140000] {
            assert!(node_type(mode).is_ok());
        }
        assert_eq!(node_type(0o040000), Err(1));
        for mode in [0o030000, 0o050000, 0o070000, 0o120000, 0o170000] {
            assert_eq!(node_type(mode), Err(22));
        }
    }
}
