//! The attributes an access check reads, whatever filesystem holds the file.
//!
//! A filesystem (ext2 today, the copy-on-write filesystem later) or a virtual
//! node (devfs, procfs) reports these three fields, and `permissions::Credentials::may`
//! decides on them alone, so no check depends on how a filesystem stores its inodes.

/// A file's type and permission bits, owner and group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InodeAttrs {
    /// `st_mode`: the file type in bits 12 to 15 and the permission bits below.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

impl InodeAttrs {
    pub const fn new(mode: u32, uid: u32, gid: u32) -> Self {
        Self { mode, uid, gid }
    }

    pub const fn is_dir(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }

    pub const fn is_regular(&self) -> bool {
        self.mode & 0o170000 == 0o100000
    }

    pub const fn is_symlink(&self) -> bool {
        self.mode & 0o170000 == 0o120000
    }

    /// Set-user-ID, set-group-ID and sticky bits with the permission bits.
    pub const fn permissions(&self) -> u32 {
        self.mode & 0o7777
    }

    /// A directory any user may create in, whose entries only their owners (or the
    /// directory's owner, or root) may remove: sticky and writable by other users.
    pub const fn is_sticky_world_writable(&self) -> bool {
        self.is_dir() && self.mode & 0o1002 == 0o1002
    }
}

/// The options a mount carries (`/proc/mounts`, statvfs `f_flag`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MountFlags {
    /// The mount refuses writes.
    pub read_only: bool,
    /// Set-user-ID and set-group-ID bits confer no identity at exec.
    pub nosuid: bool,
    /// Device nodes on the mount open no device.
    pub nodev: bool,
    /// The mount refuses exec.
    pub noexec: bool,
}

impl MountFlags {
    /// statvfs `f_flag` bits (`ST_RDONLY`, `ST_NOSUID`, `ST_NODEV`, `ST_NOEXEC`).
    pub const fn statvfs_bits(&self) -> u64 {
        (self.read_only as u64) | (self.nosuid as u64) << 1 | (self.nodev as u64) << 2 | (self.noexec as u64) << 3
    }

    /// The option list `/proc/mounts` shows, `rw` or `ro` first.
    pub fn options(&self) -> alloc::string::String {
        let mut text = alloc::string::String::from(if self.read_only { "ro" } else { "rw" });
        for (set, name) in [(self.nosuid, ",nosuid"), (self.nodev, ",nodev"), (self.noexec, ",noexec")] {
            if set {
                text.push_str(name);
            }
        }
        text
    }
}
