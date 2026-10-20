//! A kernel slot selects its retained userland and configuration tree. `/root`
//! is bound from the partition root, and so is `/.breenix-shared`, the machine
//! state store (SSH host keys and account entries) that init reconciles into
//! the generation's `/etc` at boot; each generation shares both. The
//! home mount remains a separate disk.

pub const DIRECTORY: &str = ".breenix-generations";
pub const READY: &str = ".complete";
/// The machine state store, shared by each generation and bound at `/.breenix-shared`.
pub const SHARED: &str = ".breenix-shared";

#[derive(Clone, Copy)]
pub struct Namespace {
    pub root: u32,
    pub etc: u32,
    pub data: u32,
    /// The shared machine state store, or 0 when the partition has no store.
    pub shared: u32,
}

impl Namespace {
    pub const RAW: Self = Self { root: 2, etc: 0, data: 0, shared: 0 };

    pub fn shared(self, directory: u32, name: &[u8]) -> Option<u32> {
        if directory != self.root || self.root == 2 { return None; }
        match name {
            b"etc" => Some(self.etc),
            b"root" => Some(self.data),
            b".breenix-shared" if self.shared != 0 => Some(self.shared),
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
            && (matches!(name, b"etc" | b"root" | b"home" | b"dev" | b"proc" | b"sys")
                || self.shared != 0 && name == SHARED.as_bytes())
    }

    pub fn entry(self, directory: u32, name: &[u8], inode: u32) -> Option<u32> {
        if self.hidden(directory, name) { return None; }
        if name == b".." && self.is_mount(directory) { return Some(self.root); }
        Some(self.shared(directory, name).unwrap_or(inode))
    }

    pub fn is_mount(self, ino: u32) -> bool {
        ino == self.root
            || (self.root != 2 && (ino == self.etc || ino == self.data || self.shared != 0 && ino == self.shared))
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
    // The store is bound by name, with no placeholder in the generation: lookup
    // takes the binding before the directory. A partition without one binds no store.
    let shared = tree.directory(2, SHARED).unwrap_or(0);
    Ok(Namespace { root, etc: tree.directory(root, "etc")?, data: tree.directory(2, "root")?, shared })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_root_hides_record_and_reports_bound_inodes() {
        let ns = Namespace { root: 40, etc: 41, data: 12, shared: 13 };
        assert_eq!(ns.entry(40, b".complete", 50), None);
        assert_eq!(ns.entry(40, b"etc", 99), Some(41));
        assert_eq!(ns.entry(40, b"root", 42), ns.shared(40, b"root"));
        assert_eq!(ns.entry(40, b".breenix-shared", 98), Some(13));
        assert_eq!(ns.entry(41, b".breenix-shared", 97), Some(97));
        for ino in [40, 41, 12, 13] { assert_eq!(ns.entry(ino, b"..", 2), Some(40)); }
        for name in [b"etc".as_slice(), b"root", b"home", b"dev", b"proc", b"sys", b".breenix-shared"] {
            assert!(ns.mount_entry(40, name));
            assert!(!ns.mount_entry(41, name));
            assert!(!Namespace::RAW.mount_entry(2, name));
        }
        assert!(ns.is_mount(13));
        assert_eq!(Namespace::RAW.entry(2, b".complete", 50), Some(50));
        assert_eq!(Namespace::RAW.entry(2, b".breenix-shared", 50), Some(50));
        // Without a store, an entry of the generation's own is what the name finds.
        let older = Namespace { shared: 0, ..ns };
        assert_eq!(older.entry(40, b".breenix-shared", 98), Some(98));
        assert!(!older.mount_entry(40, b".breenix-shared"));
        assert!(!older.is_mount(0));
    }
}
