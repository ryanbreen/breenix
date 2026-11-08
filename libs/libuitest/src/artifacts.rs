//! Local artifact publication: write, file sync, rename, directory sync.
use std::{fs::{self, File, OpenOptions}, io::Write, path::{Path, PathBuf}, os::unix::fs::DirBuilderExt};
use libpixcheck::sha256;
fn component(s: &str) -> bool { !s.is_empty() && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) }
fn directory(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() { directory(parent)?; }
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => { if let Some(p) = path.parent() { File::open(p)?.sync_all()?; } },
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
        Err(e) => return Err(e),
    }
    if !fs::symlink_metadata(path)?.is_dir() { return Err(std::io::Error::other("artifact path is not a directory")); }
    Ok(())
}
pub struct Store { path: PathBuf, entries: String }
impl Store {
    pub fn new(root: &Path, suite: &str, case: &str) -> Result<Self, String> {
        let parts: Vec<_> = case.split('/').collect();
        if !component(suite) || parts.len() != 2 || !parts.iter().all(|s| component(s)) { return Err("invalid artifact case path".into()); }
        directory(root).map_err(|e| e.to_string())?;
        let mut path = root.to_path_buf();
        for part in std::iter::once(suite).chain(parts.iter().copied()) {
            path.push(part); directory(&path).map_err(|e| e.to_string())?;
        }
        // A fixed case path must not inherit stale artifacts from an earlier run.
        for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() { return Err("unexpected artifact entry".into()); }
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
        File::open(&path).and_then(|d| d.sync_all()).map_err(|e| e.to_string())?;
        Ok(Self { path, entries: String::new() })
    }
    pub fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if !component(name) || name.ends_with(".tmp") { return Err("invalid artifact name".into()); }
        let tmp = self.path.join(format!("{name}.tmp"));
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            file.write_all(bytes)?; file.sync_all()?; drop(file);
            fs::rename(&tmp, self.path.join(name))?;
            File::open(&self.path)?.sync_all()
        })();
        if let Err(e) = result { let _ = fs::remove_file(tmp); return Err(format!("artifact {name}: {e}")); }
        let hash: String = sha256(bytes).iter().map(|b| format!("{b:02x}")).collect();
        self.entries.push_str(&format!("file={name} bytes={} sha256={hash}\n", bytes.len()));
        Ok(())
    }
    pub fn finish(&mut self, summary: &str) -> Result<(), String> {
        let text = format!("UTLOCAL1\n{summary}{}", self.entries);
        self.write("collection.txt", text.as_bytes())
    }
}
