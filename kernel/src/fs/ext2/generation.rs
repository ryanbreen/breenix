//! A kernel slot selects its retained userland and configuration tree. `/root`
//! is bound from the partition root; the home mount remains a separate disk.

pub const DIRECTORY: &str = ".breenix-generations";
pub const READY: &str = ".complete";

#[derive(Clone, Copy)]
pub struct Namespace {
    pub root: u32,
    pub etc: u32,
    pub data: u32,
}

impl Namespace {
    pub const RAW: Self = Self { root: 2, etc: 0, data: 0 };

    pub fn shared(self, directory: u32, name: &[u8]) -> Option<u32> {
        if directory != self.root || self.root == 2 { return None; }
        match name {
            b"etc" => Some(self.etc),
            b"root" => Some(self.data),
            _ => None,
        }
    }

    pub fn hidden(self, directory: u32, name: &[u8]) -> bool {
        self.root != 2 && directory == self.root && name == READY.as_bytes()
    }

    /// Protect the names of bind mount placeholders, whose physical inode can
    /// differ from the inode lookup exposes (notably the shared /root).
    pub fn mount_entry(self, directory: u32, name: &[u8]) -> bool {
        self.root != 2 && directory == self.root
            && matches!(name, b"etc" | b"root" | b"home" | b"dev" | b"proc" | b"sys")
    }

    pub fn entry(self, directory: u32, name: &[u8], inode: u32) -> Option<u32> {
        if self.hidden(directory, name) { return None; }
        if name == b".." && self.is_mount(directory) { return Some(self.root); }
        Some(self.shared(directory, name).unwrap_or(inode))
    }

    pub fn is_mount(self, ino: u32) -> bool {
        ino == self.root || (self.root != 2 && (ino == self.etc || ino == self.data))
    }
}

/// Raw partition access, before the selected namespace is published.
pub trait Tree {
    fn directory(&self, parent: u32, name: &str) -> Result<u32, &'static str>;
    fn contents(&self, parent: u32, name: &str) -> Result<alloc::vec::Vec<u8>, &'static str>;
}

pub fn select(tree: &impl Tree, id: &str) -> Result<Namespace, &'static str> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') { return Err("invalid userland generation"); }
    let generations = tree.directory(2, DIRECTORY)?;
    let root = tree.directory(generations, id)?;
    let expected = alloc::format!("BREENIX-USERLAND 1\n{}\n", id);
    if tree.contents(root, READY)? != expected.as_bytes() {
        return Err("incomplete userland generation");
    }
    // Require directory mount points so selection does not follow symlinks
    // out of the generation.
    for name in ["etc", "root", "home", "dev", "proc"] { tree.directory(root, name)?; }
    Ok(Namespace { root, etc: tree.directory(root, "etc")?, data: tree.directory(2, "root")? })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_root_hides_record_and_reports_bound_inodes() {
        let ns = Namespace { root: 40, etc: 41, data: 12 };
        assert_eq!(ns.entry(40, b".complete", 50), None);
        assert_eq!(ns.entry(40, b"etc", 41), ns.shared(40, b"etc"));
        assert_eq!(ns.entry(40, b"root", 42), ns.shared(40, b"root"));
        for ino in [40, 41, 12] { assert_eq!(ns.entry(ino, b"..", 2), Some(40)); }
        for name in [b"etc".as_slice(), b"root", b"home", b"dev", b"proc", b"sys"] {
            assert!(ns.mount_entry(40, name));
            assert!(!ns.mount_entry(41, name));
            assert!(!Namespace::RAW.mount_entry(2, name));
        }
        assert_eq!(Namespace::RAW.entry(2, b".complete", 50), Some(50));
    }
}
