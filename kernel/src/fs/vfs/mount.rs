//! VFS Mount Point Management
//!
//! Manages filesystem mount points and the global mount table.

use super::error::VfsError;
pub use crate::fs::attrs::MountFlags;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// A mounted filesystem
#[derive(Debug)]
pub struct MountPoint {
    /// Path where this filesystem is mounted (e.g., "/", "/mnt/data")
    pub mount_path: String,
    /// Unique mount ID
    pub mount_id: usize,
    /// Filesystem type (e.g., "ext2", "tmpfs")
    pub fs_type: &'static str,
    /// The mount's options.
    pub flags: MountFlags,
    // Future: Will hold Arc<dyn Filesystem> for actual filesystem operations
}

/// `/home`'s options: users' files there confer no identity at exec and open
/// no device (`docs/users/design.md`, section 2).
pub const HOME_FLAGS: MountFlags = MountFlags { read_only: false, nosuid: true, nodev: true, noexec: false };

/// Global mount table
static MOUNT_TABLE: Mutex<Vec<MountPoint>> = Mutex::new(Vec::new());

/// Next available mount ID
static NEXT_MOUNT_ID: Mutex<usize> = Mutex::new(0);

/// Register a mount point
///
/// # Arguments
/// * `path` - The path to mount at (e.g., "/", "/mnt/data")
/// * `fs_type` - The filesystem type (e.g., "ext2", "tmpfs")
///
/// # Returns
/// The mount ID for this mount point
#[allow(dead_code)] // Called from ext2::init_root_fs which may not be invoked in all configs
pub fn mount(path: &str, fs_type: &'static str) -> usize {
    mount_with(path, fs_type, MountFlags::default())
}

/// Register a mount point with options.
pub fn mount_with(path: &str, fs_type: &'static str, flags: MountFlags) -> usize {
    let mut table = MOUNT_TABLE.lock();
    let mut next_id = NEXT_MOUNT_ID.lock();

    let mount_id = *next_id;
    *next_id += 1;

    table.push(MountPoint {
        mount_path: String::from(path),
        mount_id,
        fs_type,
        flags,
    });

    mount_id
}

/// Mount `/home` with `HOME_FLAGS` over the root filesystem's own `/home`
/// directory when no home filesystem was mounted there, so the options hold
/// for `/home` whichever filesystem holds it.
pub fn bind_home_if_unmounted() {
    if !MOUNT_TABLE.lock().iter().any(|m| m.mount_path == "/home") {
        mount_with("/home", "ext2", HOME_FLAGS);
    }
}

/// The options of the mount a physical absolute pathname lies on: the mount
/// whose path is the pathname or its longest leading run of whole components.
pub fn flags_at(path: &[u8]) -> MountFlags {
    let table = MOUNT_TABLE.lock();
    let mut best: Option<&MountPoint> = None;
    for mount in table.iter() {
        let prefix = mount.mount_path.as_bytes();
        let within = prefix == b"/"
            || (path.starts_with(prefix) && matches!(path.get(prefix.len()), None | Some(b'/')));
        if within && best.is_none_or(|b| prefix.len() > b.mount_path.len()) {
            best = Some(mount);
        }
    }
    best.map(|m| m.flags).unwrap_or_default()
}

/// Unmount a filesystem
///
/// # Arguments
/// * `mount_id` - The mount ID to unmount
///
/// # Returns
/// Ok(()) if successful, VfsError if the mount point doesn't exist
#[allow(dead_code)] // Part of VFS mount API
pub fn unmount(mount_id: usize) -> Result<(), VfsError> {
    let mut table = MOUNT_TABLE.lock();

    if let Some(pos) = table.iter().position(|m| m.mount_id == mount_id) {
        table.remove(pos);
        Ok(())
    } else {
        Err(VfsError::NotMounted)
    }
}

/// Find the mount point for a given path
///
/// Finds the most specific (longest matching) mount point for a path.
/// For example, if "/" and "/mnt/data" are mounted, the path "/mnt/data/file.txt"
/// will match "/mnt/data".
///
/// # Arguments
/// * `path` - The path to lookup
///
/// # Returns
/// The mount ID if found, None otherwise
#[allow(dead_code)] // Part of VFS mount API
pub fn find_mount(path: &str) -> Option<usize> {
    let table = MOUNT_TABLE.lock();

    // Find the longest matching mount path
    let mut best_match: Option<(usize, usize)> = None; // (mount_id, path_len)

    for mount in table.iter() {
        if path.starts_with(&mount.mount_path) {
            let path_len = mount.mount_path.len();
            if let Some((_, current_len)) = best_match {
                if path_len > current_len {
                    best_match = Some((mount.mount_id, path_len));
                }
            } else {
                best_match = Some((mount.mount_id, path_len));
            }
        }
    }

    best_match.map(|(mount_id, _)| mount_id)
}

/// Get information about a mount point
///
/// # Arguments
/// * `mount_id` - The mount ID to query
///
/// # Returns
/// A tuple of (mount_path, fs_type) if found, None otherwise
#[allow(dead_code)] // Part of VFS mount API
pub fn get_mount_info(mount_id: usize) -> Option<(String, &'static str)> {
    let table = MOUNT_TABLE.lock();
    table
        .iter()
        .find(|m| m.mount_id == mount_id)
        .map(|m| (m.mount_path.clone(), m.fs_type))
}

/// List all mount points
///
/// # Returns
/// A vector of (mount_id, mount_path, fs_type, options) tuples
#[allow(dead_code)] // Part of VFS mount API
pub fn list_mounts() -> Vec<(usize, String, &'static str, MountFlags)> {
    let table = MOUNT_TABLE.lock();
    table
        .iter()
        .map(|m| (m.mount_id, m.mount_path.clone(), m.fs_type, m.flags))
        .collect()
}

/// Final shutdown fence, also visible through /proc/mounts and statvfs.
pub fn remount_read_only(id: usize) -> Result<(), &'static str> {
    let mut table = MOUNT_TABLE.lock();
    let mount = table.iter_mut().find(|m| m.mount_id == id).ok_or("Mount disappeared")?;
    mount.flags.read_only = true;
    Ok(())
}
