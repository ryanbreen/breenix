//! GUI case custody of global configuration. Snapshots read symlink targets as names, without following them.
use std::{ffi::OsString, fs, io, path::{Path, PathBuf}};
use std::os::unix::{ffi::OsStrExt, fs::{MetadataExt, PermissionsExt, symlink}};
use libpixcheck::Sha256;

pub const GLOBAL_PATHS: [&str; 4] = [
    "/etc/fonts.conf", "/etc/hotkeys.conf", "/etc/bwm.conf",
    "/usr/share/applications",
];
#[derive(Clone)]
struct Attributes { mode: u32, uid: u32, gid: u32 }
#[derive(Clone)]
enum Contents { Missing, File(Vec<u8>), Directory(Vec<(OsString, Node)>), Link(PathBuf) }
#[derive(Clone)]
struct Node { attrs: Attributes, contents: Contents }
fn missing(error: &io::Error) -> bool { error.kind() == io::ErrorKind::NotFound }
impl Node {
    fn read(path: &Path) -> io::Result<Self> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if missing(&e) => return Ok(Self { attrs: Attributes { mode: 0, uid: 0, gid: 0 }, contents: Contents::Missing }),
            Err(e) => return Err(e),
        };
        let attrs = Attributes { mode: meta.mode() & 0o7777, uid: meta.uid(), gid: meta.gid() };
        let contents = if meta.is_symlink() { Contents::Link(fs::read_link(path)?) }
        else if meta.is_file() { Contents::File(fs::read(path)?) }
        else if meta.is_dir() {
            let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
            entries.sort_by_key(|e| e.file_name());
            let mut children = Vec::new();
            for entry in entries { children.push((entry.file_name(), Self::read(&entry.path())?)); }
            Contents::Directory(children)
        } else { return Err(io::Error::other("unsupported configuration file type")); };
        Ok(Self { attrs, contents })
    }
    fn hash(&self, hash: &mut Sha256) {
        let tag = match self.contents { Contents::Missing => 0, Contents::File(_) => 1, Contents::Directory(_) => 2, Contents::Link(_) => 3 };
        hash.update(&[tag]);
        for n in [self.attrs.mode, self.attrs.uid, self.attrs.gid] { hash.update(&n.to_le_bytes()); }
        let bytes = |hash: &mut Sha256, bytes: &[u8]| { hash.update(&(bytes.len() as u64).to_le_bytes()); hash.update(bytes); };
        match &self.contents {
            Contents::Missing => {},
            Contents::File(data) => bytes(hash, data),
            Contents::Link(target) => bytes(hash, target.as_os_str().as_bytes()),
            Contents::Directory(children) => {
                hash.update(&(children.len() as u64).to_le_bytes());
                for (name, child) in children { bytes(hash, name.as_bytes()); child.hash(hash); }
            }
        }
    }
    fn digest(&self) -> [u8; 32] { let mut h = Sha256::new(); self.hash(&mut h); h.finish() }
    fn remove(path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                // A case may have made the directory unsearchable/read-only.
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
                for entry in fs::read_dir(path)? { Self::remove(&entry?.path())?; }
                fs::remove_dir(path)
            }
            Ok(_) => fs::remove_file(path),
            Err(e) if missing(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn write(&self, path: &Path) -> io::Result<()> {
        match &self.contents {
            Contents::Missing => return Ok(()),
            Contents::File(data) => {
                use std::io::Write;
                let mut file = fs::File::create(path)?; file.write_all(data)?; file.sync_all()?;
            }
            Contents::Link(target) => symlink(target, path)?,
            Contents::Directory(children) => {
                fs::create_dir(path)?;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
                for (name, child) in children { child.write(&path.join(name))?; }
            }
        }
        let meta = fs::symlink_metadata(path)?;
        if meta.uid() != self.attrs.uid || meta.gid() != self.attrs.gid {
            extern "C" { fn lchown(path: *const std::ffi::c_char, uid: u32, gid: u32) -> i32; }
            let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            if unsafe { lchown(name.as_ptr(), self.attrs.uid, self.attrs.gid) } != 0 { return Err(io::Error::last_os_error()); }
        }
        if !matches!(self.contents, Contents::Link(_)) { fs::set_permissions(path, fs::Permissions::from_mode(self.attrs.mode))?; }
        Ok(())
    }
}
/// Held by the runner, outside the case and its supervisor, until cleanup ends.
pub struct Snapshot { paths: Vec<(&'static str, PathBuf, Node, [u8; 32])> }
impl Snapshot {
    pub fn capture() -> Result<Self, String> { Self::capture_at(Path::new("/")) }
    pub fn capture_at(root: &Path) -> Result<Self, String> {
        let mut paths = Vec::new();
        for name in GLOBAL_PATHS {
            let path = root.join(&name[1..]);
            let node = Node::read(&path).map_err(|e| format!("config snapshot {name}: {e}"))?;
            let digest = node.digest();
            paths.push((name, path, node, digest));
        }
        Ok(Self { paths })
    }
    /// Check after descendants are gone, restore changed roots, then report them.
    /// A read failure also triggers restoration; it is treated as a difference.
    pub fn verify_restore(self) -> Result<(), String> {
        let mut changes = Vec::new();
        let mut errors = Vec::new();
        for (name, path, before, digest) in self.paths {
            match Node::read(&path) {
                Ok(after) if after.digest() == digest => continue,
                _ => {},
            }
            let restored = Node::remove(&path).and_then(|()| before.write(&path))
                .and_then(|()| {
                    if Node::read(&path)?.digest() == digest { Ok(()) }
                    else { Err(io::Error::other("restored hash differs")) }
                });
            changes.push(name);
            if let Err(e) = restored { errors.push(format!("{name}: {e}")); }
        }
        if changes.is_empty() { return Ok(()); }
        let result = if errors.is_empty() { "restored".into() } else { format!("restore failed: {}", errors.join("; ")) };
        // List paths first so the runner's bounded serial message keeps their names.
        Err(format!("global config changed: {}; {result}", changes.join(", ")))
    }
}
