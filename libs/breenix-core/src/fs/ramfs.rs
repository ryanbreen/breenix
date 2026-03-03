//! In-Memory Filesystem (ramfs)
//!
//! Provides a fully in-memory filesystem implementation using BTreeMap.
//! Useful as tmpfs in the kernel and as the boot filesystem for WASM targets.
//!
//! Supports: files, directories, device nodes, symlinks.
//! Path resolution with `.` and `..` normalization.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::fs::vfs::{VfsError, FileType, FilePermissions, VfsInode, DirEntry, FileStat};

/// A single inode in the RAM filesystem
#[derive(Debug, Clone)]
struct RamInode {
    /// Inode number
    inode_num: u64,
    /// File type
    file_type: FileType,
    /// File permissions
    permissions: FilePermissions,
    /// Owner user ID
    uid: u32,
    /// Owner group ID
    gid: u32,
    /// Number of hard links
    link_count: u16,
    /// Timestamps
    atime: u64,
    mtime: u64,
    ctime: u64,
    /// File content (for regular files)
    data: Vec<u8>,
    /// Directory entries: name -> inode_num (for directories)
    children: BTreeMap<String, u64>,
    /// Symlink target (for symlinks)
    symlink_target: Option<String>,
    /// Device info (for device nodes): (major, minor)
    device: Option<(u32, u32)>,
    /// Parent inode number
    parent: u64,
}

impl RamInode {
    fn new(inode_num: u64, file_type: FileType, permissions: FilePermissions, parent: u64) -> Self {
        Self {
            inode_num,
            file_type,
            permissions,
            uid: 0,
            gid: 0,
            link_count: if file_type == FileType::Directory { 2 } else { 1 },
            atime: 0,
            mtime: 0,
            ctime: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            symlink_target: None,
            device: None,
            parent,
        }
    }

    fn size(&self) -> u64 {
        match self.file_type {
            FileType::Regular => self.data.len() as u64,
            FileType::Directory => 4096,
            FileType::SymLink => self.symlink_target.as_ref().map_or(0, |s| s.len() as u64),
            _ => 0,
        }
    }

    fn to_vfs_inode(&self) -> VfsInode {
        VfsInode {
            inode_num: self.inode_num,
            file_type: self.file_type,
            size: self.size(),
            permissions: self.permissions,
            uid: self.uid,
            gid: self.gid,
            link_count: self.link_count,
            atime: self.atime,
            mtime: self.mtime,
            ctime: self.ctime,
        }
    }

    fn to_stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            mode: self.permissions.to_mode(),
            file_type: self.file_type,
            nlink: self.link_count,
            uid: self.uid,
            gid: self.gid,
            size: self.size(),
            atime: self.atime,
            mtime: self.mtime,
            ctime: self.ctime,
            rdev: self.device.map_or(0, |(maj, min)| ((maj as u64) << 8) | (min as u64)),
        }
    }
}

/// In-memory filesystem
pub struct RamFs {
    /// All inodes, keyed by inode number
    inodes: BTreeMap<u64, RamInode>,
    /// Next inode number to allocate
    next_inode: u64,
}

impl RamFs {
    /// Create a new empty RAM filesystem with a root directory
    pub fn new() -> Self {
        let root_perms = FilePermissions::from_mode(0o755);
        let mut root = RamInode::new(1, FileType::Directory, root_perms, 1);
        root.children.insert(String::from("."), 1);
        root.children.insert(String::from(".."), 1);

        let mut inodes = BTreeMap::new();
        inodes.insert(1, root);

        Self {
            inodes,
            next_inode: 2,
        }
    }

    /// Root inode number
    pub const ROOT_INODE: u64 = 1;

    fn alloc_inode(&mut self) -> u64 {
        let num = self.next_inode;
        self.next_inode += 1;
        num
    }

    /// Normalize a path: resolve `.`, `..`, collapse multiple slashes
    fn normalize_path(path: &str) -> String {
        let mut components: Vec<&str> = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => { components.pop(); }
                name => { components.push(name); }
            }
        }
        if components.is_empty() {
            String::from("/")
        } else {
            let mut result = String::new();
            for c in &components {
                result.push('/');
                result.push_str(c);
            }
            result
        }
    }

    /// Resolve a path to an inode number, starting from root
    pub fn resolve_path(&self, path: &str) -> Result<u64, VfsError> {
        let normalized = Self::normalize_path(path);
        if normalized == "/" {
            return Ok(Self::ROOT_INODE);
        }

        let mut current = Self::ROOT_INODE;
        for component in normalized.trim_start_matches('/').split('/') {
            if component.is_empty() {
                continue;
            }
            let inode = self.inodes.get(&current).ok_or(VfsError::NotFound)?;
            if inode.file_type != FileType::Directory {
                return Err(VfsError::NotDirectory);
            }
            current = *inode.children.get(component).ok_or(VfsError::NotFound)?;

            // Follow symlinks
            let target = self.inodes.get(&current).ok_or(VfsError::NotFound)?;
            if target.file_type == FileType::SymLink {
                if let Some(ref link_target) = target.symlink_target {
                    current = self.resolve_path(link_target)?;
                }
            }
        }
        Ok(current)
    }

    /// Resolve a path, returning (parent_inode, filename)
    fn resolve_parent(&self, path: &str) -> Result<(u64, String), VfsError> {
        let normalized = Self::normalize_path(path);
        if normalized == "/" {
            return Err(VfsError::AlreadyExists);
        }
        let parts: Vec<&str> = normalized.trim_start_matches('/').split('/').collect();
        if parts.is_empty() {
            return Err(VfsError::InvalidPath);
        }
        let filename = String::from(*parts.last().unwrap());
        let parent_path = if parts.len() == 1 {
            String::from("/")
        } else {
            let mut p = String::new();
            for part in &parts[..parts.len() - 1] {
                p.push('/');
                p.push_str(part);
            }
            p
        };
        let parent_ino = self.resolve_path(&parent_path)?;
        Ok((parent_ino, filename))
    }

    /// Create a regular file at the given path
    pub fn create_file(&mut self, path: &str, permissions: u16) -> Result<u64, VfsError> {
        let (parent_ino, filename) = self.resolve_parent(path)?;

        // Check parent is a directory
        let parent = self.inodes.get(&parent_ino).ok_or(VfsError::NotFound)?;
        if parent.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        // Check for existing entry
        if parent.children.contains_key(&filename) {
            return Err(VfsError::AlreadyExists);
        }

        let ino = self.alloc_inode();
        let inode = RamInode::new(ino, FileType::Regular, FilePermissions::from_mode(permissions), parent_ino);
        self.inodes.insert(ino, inode);

        // Add to parent's children
        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.insert(filename, ino);

        Ok(ino)
    }

    /// Create a directory at the given path
    pub fn mkdir(&mut self, path: &str, permissions: u16) -> Result<u64, VfsError> {
        let (parent_ino, dirname) = self.resolve_parent(path)?;

        let parent = self.inodes.get(&parent_ino).ok_or(VfsError::NotFound)?;
        if parent.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        if parent.children.contains_key(&dirname) {
            return Err(VfsError::AlreadyExists);
        }

        let ino = self.alloc_inode();
        let mut inode = RamInode::new(ino, FileType::Directory, FilePermissions::from_mode(permissions), parent_ino);
        inode.children.insert(String::from("."), ino);
        inode.children.insert(String::from(".."), parent_ino);
        self.inodes.insert(ino, inode);

        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.insert(dirname, ino);
        parent.link_count += 1;

        Ok(ino)
    }

    /// Create directories recursively (like mkdir -p)
    pub fn mkdir_p(&mut self, path: &str, permissions: u16) -> Result<u64, VfsError> {
        let normalized = Self::normalize_path(path);
        if normalized == "/" {
            return Ok(Self::ROOT_INODE);
        }

        let parts: Vec<&str> = normalized.trim_start_matches('/').split('/').collect();
        let mut current_path = String::new();

        let mut last_ino = Self::ROOT_INODE;
        for part in &parts {
            current_path.push('/');
            current_path.push_str(part);
            match self.resolve_path(&current_path) {
                Ok(ino) => {
                    last_ino = ino;
                }
                Err(VfsError::NotFound) => {
                    last_ino = self.mkdir(&current_path, permissions)?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(last_ino)
    }

    /// Read file contents
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let ino = self.resolve_path(path)?;
        let inode = self.inodes.get(&ino).ok_or(VfsError::NotFound)?;
        if inode.file_type != FileType::Regular {
            return Err(VfsError::IsDirectory);
        }
        Ok(inode.data.clone())
    }

    /// Read file contents by inode number
    pub fn read_file_by_inode(&self, ino: u64) -> Result<Vec<u8>, VfsError> {
        let inode = self.inodes.get(&ino).ok_or(VfsError::NotFound)?;
        if inode.file_type != FileType::Regular {
            return Err(VfsError::IsDirectory);
        }
        Ok(inode.data.clone())
    }

    /// Write file contents (overwrites)
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        let ino = self.resolve_path(path)?;
        let inode = self.inodes.get_mut(&ino).ok_or(VfsError::NotFound)?;
        if inode.file_type != FileType::Regular {
            return Err(VfsError::IsDirectory);
        }
        inode.data = data.to_vec();
        Ok(())
    }

    /// Write file contents by inode number
    pub fn write_file_by_inode(&mut self, ino: u64, data: &[u8]) -> Result<(), VfsError> {
        let inode = self.inodes.get_mut(&ino).ok_or(VfsError::NotFound)?;
        if inode.file_type != FileType::Regular {
            return Err(VfsError::IsDirectory);
        }
        inode.data = data.to_vec();
        Ok(())
    }

    /// Append to file contents
    pub fn append_file(&mut self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        let ino = self.resolve_path(path)?;
        let inode = self.inodes.get_mut(&ino).ok_or(VfsError::NotFound)?;
        if inode.file_type != FileType::Regular {
            return Err(VfsError::IsDirectory);
        }
        inode.data.extend_from_slice(data);
        Ok(())
    }

    /// List directory entries
    pub fn readdir(&self, path: &str) -> Result<Vec<DirEntry>, VfsError> {
        let ino = self.resolve_path(path)?;
        let inode = self.inodes.get(&ino).ok_or(VfsError::NotFound)?;
        if inode.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }

        let mut entries = Vec::new();
        for (name, &child_ino) in &inode.children {
            let child = self.inodes.get(&child_ino);
            let file_type = child.map_or(FileType::Regular, |c| c.file_type);
            entries.push(DirEntry {
                name: name.clone(),
                inode_num: child_ino,
                file_type,
            });
        }
        Ok(entries)
    }

    /// Remove a file (unlink)
    pub fn unlink(&mut self, path: &str) -> Result<(), VfsError> {
        let (parent_ino, filename) = self.resolve_parent(path)?;

        let child_ino = {
            let parent = self.inodes.get(&parent_ino).ok_or(VfsError::NotFound)?;
            *parent.children.get(&filename).ok_or(VfsError::NotFound)?
        };

        // Check it's not a directory (use rmdir for that)
        let child = self.inodes.get(&child_ino).ok_or(VfsError::NotFound)?;
        if child.file_type == FileType::Directory {
            return Err(VfsError::IsDirectory);
        }

        // Remove from parent
        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.remove(&filename);

        // Remove inode (simplification: no hard link tracking)
        self.inodes.remove(&child_ino);
        Ok(())
    }

    /// Remove an empty directory
    pub fn rmdir(&mut self, path: &str) -> Result<(), VfsError> {
        let (parent_ino, dirname) = self.resolve_parent(path)?;

        let child_ino = {
            let parent = self.inodes.get(&parent_ino).ok_or(VfsError::NotFound)?;
            *parent.children.get(&dirname).ok_or(VfsError::NotFound)?
        };

        let child = self.inodes.get(&child_ino).ok_or(VfsError::NotFound)?;
        if child.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }

        // Check empty (only . and ..)
        let real_entries: usize = child.children.keys()
            .filter(|k| k.as_str() != "." && k.as_str() != "..")
            .count();
        if real_entries > 0 {
            return Err(VfsError::IoError); // ENOTEMPTY
        }

        // Remove from parent
        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.remove(&dirname);
        parent.link_count = parent.link_count.saturating_sub(1);

        // Remove inode
        self.inodes.remove(&child_ino);
        Ok(())
    }

    /// Rename/move a file or directory
    pub fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), VfsError> {
        let (old_parent_ino, old_name) = self.resolve_parent(old_path)?;
        let (new_parent_ino, new_name) = self.resolve_parent(new_path)?;

        // Get the inode being moved
        let child_ino = {
            let parent = self.inodes.get(&old_parent_ino).ok_or(VfsError::NotFound)?;
            *parent.children.get(&old_name).ok_or(VfsError::NotFound)?
        };

        // Remove from old parent
        let old_parent = self.inodes.get_mut(&old_parent_ino).unwrap();
        old_parent.children.remove(&old_name);

        // If destination exists, remove it first
        let existing_to_remove = self
            .inodes
            .get(&new_parent_ino)
            .and_then(|p| p.children.get(&new_name).copied());
        if let Some(existing_ino) = existing_to_remove {
            self.inodes.remove(&existing_ino);
        }
        let new_parent = self.inodes.get_mut(&new_parent_ino).unwrap();
        new_parent.children.insert(new_name, child_ino);

        // Update parent reference for the moved inode
        if let Some(inode) = self.inodes.get_mut(&child_ino) {
            inode.parent = new_parent_ino;
            if inode.file_type == FileType::Directory {
                inode.children.insert(String::from(".."), new_parent_ino);
            }
        }

        Ok(())
    }

    /// Get file/directory stat information
    pub fn stat(&self, path: &str) -> Result<FileStat, VfsError> {
        let ino = self.resolve_path(path)?;
        let inode = self.inodes.get(&ino).ok_or(VfsError::NotFound)?;
        Ok(inode.to_stat())
    }

    /// Get VFS inode for a path
    pub fn inode(&self, path: &str) -> Result<VfsInode, VfsError> {
        let ino = self.resolve_path(path)?;
        let inode = self.inodes.get(&ino).ok_or(VfsError::NotFound)?;
        Ok(inode.to_vfs_inode())
    }

    /// Check if a path exists
    pub fn exists(&self, path: &str) -> bool {
        self.resolve_path(path).is_ok()
    }

    /// Check if a path is a directory
    pub fn is_dir(&self, path: &str) -> bool {
        self.resolve_path(path)
            .and_then(|ino| self.inodes.get(&ino).ok_or(VfsError::NotFound))
            .map_or(false, |inode| inode.file_type == FileType::Directory)
    }

    /// Create a symlink
    pub fn symlink(&mut self, path: &str, target: &str) -> Result<u64, VfsError> {
        let (parent_ino, name) = self.resolve_parent(path)?;

        let parent = self.inodes.get(&parent_ino).ok_or(VfsError::NotFound)?;
        if parent.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        if parent.children.contains_key(&name) {
            return Err(VfsError::AlreadyExists);
        }

        let ino = self.alloc_inode();
        let mut inode = RamInode::new(ino, FileType::SymLink, FilePermissions::from_mode(0o777), parent_ino);
        inode.symlink_target = Some(String::from(target));
        self.inodes.insert(ino, inode);

        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.insert(name, ino);

        Ok(ino)
    }

    /// Create a device node
    pub fn mknod(&mut self, path: &str, file_type: FileType, major: u32, minor: u32) -> Result<u64, VfsError> {
        let (parent_ino, name) = self.resolve_parent(path)?;

        let parent = self.inodes.get(&parent_ino).ok_or(VfsError::NotFound)?;
        if parent.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        if parent.children.contains_key(&name) {
            return Err(VfsError::AlreadyExists);
        }

        let ino = self.alloc_inode();
        let mut inode = RamInode::new(ino, file_type, FilePermissions::from_mode(0o666), parent_ino);
        inode.device = Some((major, minor));
        self.inodes.insert(ino, inode);

        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.insert(name, ino);

        Ok(ino)
    }

    /// Create a file or return its inode if it already exists
    pub fn create_or_get(&mut self, path: &str, permissions: u16) -> Result<u64, VfsError> {
        match self.resolve_path(path) {
            Ok(ino) => Ok(ino),
            Err(VfsError::NotFound) => self.create_file(path, permissions),
            Err(e) => Err(e),
        }
    }

    /// Truncate a file to zero length
    pub fn truncate(&mut self, path: &str) -> Result<(), VfsError> {
        let ino = self.resolve_path(path)?;
        let inode = self.inodes.get_mut(&ino).ok_or(VfsError::NotFound)?;
        if inode.file_type != FileType::Regular {
            return Err(VfsError::IsDirectory);
        }
        inode.data.clear();
        Ok(())
    }
}

impl Default for RamFs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_read_file() {
        let mut fs = RamFs::new();
        fs.create_file("/hello.txt", 0o644).unwrap();
        fs.write_file("/hello.txt", b"Hello, World!").unwrap();
        let data = fs.read_file("/hello.txt").unwrap();
        assert_eq!(data, b"Hello, World!");
    }

    #[test]
    fn test_mkdir_and_readdir() {
        let mut fs = RamFs::new();
        fs.mkdir("/bin", 0o755).unwrap();
        fs.mkdir("/etc", 0o755).unwrap();

        let entries = fs.readdir("/").unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"bin"));
        assert!(names.contains(&"etc"));
    }

    #[test]
    fn test_mkdir_p() {
        let mut fs = RamFs::new();
        fs.mkdir_p("/a/b/c/d", 0o755).unwrap();
        assert!(fs.is_dir("/a"));
        assert!(fs.is_dir("/a/b"));
        assert!(fs.is_dir("/a/b/c"));
        assert!(fs.is_dir("/a/b/c/d"));
    }

    #[test]
    fn test_unlink() {
        let mut fs = RamFs::new();
        fs.create_file("/test.txt", 0o644).unwrap();
        assert!(fs.exists("/test.txt"));
        fs.unlink("/test.txt").unwrap();
        assert!(!fs.exists("/test.txt"));
    }

    #[test]
    fn test_rmdir() {
        let mut fs = RamFs::new();
        fs.mkdir("/empty", 0o755).unwrap();
        fs.rmdir("/empty").unwrap();
        assert!(!fs.exists("/empty"));
    }

    #[test]
    fn test_rmdir_not_empty() {
        let mut fs = RamFs::new();
        fs.mkdir("/dir", 0o755).unwrap();
        fs.create_file("/dir/file.txt", 0o644).unwrap();
        assert!(fs.rmdir("/dir").is_err());
    }

    #[test]
    fn test_rename() {
        let mut fs = RamFs::new();
        fs.create_file("/old.txt", 0o644).unwrap();
        fs.write_file("/old.txt", b"data").unwrap();
        fs.rename("/old.txt", "/new.txt").unwrap();
        assert!(!fs.exists("/old.txt"));
        assert_eq!(fs.read_file("/new.txt").unwrap(), b"data");
    }

    #[test]
    fn test_path_normalization() {
        assert_eq!(RamFs::normalize_path("/"), "/");
        assert_eq!(RamFs::normalize_path("//"), "/");
        assert_eq!(RamFs::normalize_path("/a/./b/../c"), "/a/c");
        assert_eq!(RamFs::normalize_path("/a/b/../../c"), "/c");
    }

    #[test]
    fn test_stat() {
        let mut fs = RamFs::new();
        fs.create_file("/test.txt", 0o644).unwrap();
        fs.write_file("/test.txt", b"hello").unwrap();
        let stat = fs.stat("/test.txt").unwrap();
        assert_eq!(stat.size, 5);
        assert_eq!(stat.file_type, FileType::Regular);
    }

    #[test]
    fn test_append() {
        let mut fs = RamFs::new();
        fs.create_file("/log.txt", 0o644).unwrap();
        fs.write_file("/log.txt", b"line1\n").unwrap();
        fs.append_file("/log.txt", b"line2\n").unwrap();
        let data = fs.read_file("/log.txt").unwrap();
        assert_eq!(data, b"line1\nline2\n");
    }
}
