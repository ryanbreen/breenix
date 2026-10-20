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
