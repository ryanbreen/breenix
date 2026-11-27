//! Userland flash preflight and carry-forward. Shared and mounted files are
//! outside the generation tree.
use super::{fail, layout, present, require_directory, Copier, KEPT, READY, UNTOUCHED};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{self, File, Metadata},
    io::Read,
    os::unix::{ffi::OsStrExt, fs::{DirBuilderExt, MetadataExt, PermissionsExt}},
    path::{Component, Path, PathBuf},
};

#[cfg(not(target_os = "breenix"))]
#[allow(dead_code)] // The standalone host regression only uses the streaming interface.
#[path = "../../../../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;
#[cfg(target_os = "breenix")]
use libbreenix::crypto::sha256;

const BASELINE: &str = ".breenix-image";
const VERSION: &str = "BREENIX-IMAGE-1";
const ARCHIVE: &str = ".breenix-carried";
const GENERATION: &str = "usr/share/factory/generation";

pub(super) fn outside_image(name: &str) -> bool {
    reserved(name) || layout::SHARED.contains(&name)
}

fn reserved(name: &str) -> bool {
    matches!(name, BASELINE | ARCHIVE) || UNTOUCHED.contains(&name) || KEPT.contains(&name)
}

fn top_level(path: &Path) -> &str {
    path.components().next().and_then(|name| name.as_os_str().to_str()).unwrap_or_default()
}

fn fingerprint(path: &Path, meta: &Metadata) -> Result<String, String> {
    if meta.is_dir() {
        // Paths in a directory are compared separately. Adding an entry does
        // not make the entire directory a local change.
        return Ok("directory".into());
    }
    let mut hash = sha256::Sha256::new();
    for value in [meta.mode(), meta.uid(), meta.gid()] {
        hash.update(&value.to_le_bytes());
    }
    if meta.is_file() {
        let mut file = File::open(path).map_err(|error| fail("open", path, error))?;
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|error| fail("read", path, error))?;
            if read == 0 { break; }
            hash.update(&buffer[..read]);
        }
    } else if meta.is_symlink() {
        hash.update(fs::read_link(path).map_err(|error| fail("readlink", path, error))?
                    .as_os_str().as_bytes());
    } else {
        return Ok("unsupported".into());
    }
    let mut result = String::new();
    for byte in hash.finalize() {
        write!(result, "{byte:02x}").expect("formatting into a string cannot fail");
    }
    Ok(result)
}

fn entries(root: &Path, scratch: Option<&Path>, legacy_shared: bool) -> Result<BTreeMap<PathBuf, String>, String> {
    require_directory(root)?;
    let mut result = BTreeMap::new();
    let mut dirs = vec![PathBuf::new()];
    while let Some(dir) = dirs.pop() {
        let parent = root.join(&dir);
        for entry in fs::read_dir(&parent).map_err(|error| fail("read directory", &parent, error))? {
            let path = dir.join(entry.map_err(|error| fail("read entry", &parent, error))?.file_name());
            let name = top_level(&path);
            if path == Path::new(GENERATION) || reserved(name) || scratch == Some(path.as_path())
                || !legacy_shared && layout::SHARED.contains(&name)
            {
                continue;
            }
            let file = root.join(&path);
            let meta = fs::symlink_metadata(&file).map_err(|error| fail("stat", &file, error))?;
            if meta.is_dir() { dirs.push(path.clone()); }
            result.insert(path, fingerprint(&file, &meta)?);
        }
    }
    Ok(result)
}

pub(super) fn record(copier: &mut Copier, staged: &Path, scratch: Option<&Path>) -> Result<(), String> {
    let mut text = format!("{VERSION}\0");
    for (path, digest) in entries(staged, scratch, false)? {
        let path = path.to_str().ok_or("image contains a non-UTF-8 path")?;
        write!(text, "{digest}\t{path}\0").expect("formatting into a string cannot fail");
    }
    copier.write_file(&staged.join(BASELINE), &text, 0o600)
}

fn read_baseline(current: &Path) -> Result<Option<BTreeMap<PathBuf, String>>, String> {
    let path = current.join(BASELINE);
    let meta = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        result => result.map_err(|error| fail("stat image baseline", &path, error))?,
    };
    if !meta.is_file() {
        return Err("image baseline is not a regular file".into());
    }
    let text = fs::read_to_string(&path).map_err(|error| fail("read image baseline", &path, error))?;
    let text = text.strip_prefix(&format!("{VERSION}\0")).ok_or("invalid image baseline header")?;
    let mut files = BTreeMap::new();
    if !text.is_empty() && !text.ends_with('\0') {
        return Err("truncated image baseline".into());
    }
    for row in text.split_terminator('\0') {
        let (digest, path) = row.split_once('\t').ok_or("invalid image baseline entry")?;
        let path = PathBuf::from(path);
        if path.as_os_str().is_empty() || !path.components().all(|part| matches!(part, Component::Normal(_)))
            || files.insert(path, digest.to_owned()).is_some()
        {
            return Err("invalid image baseline path".into());
        }
    }
    Ok(Some(files))
}

fn breenfs_payload(path: &Path) -> bool {
    matches!(top_level(path), "bin" | "sbin" | "usr")
}

/// Traverse the incoming tree without following a symlink in a parent.
fn incoming(root: &Path, path: &Path, breenfs: bool) -> Result<Option<String>, String> {
    if breenfs && !breenfs_payload(path) {
        return Ok(None);
    }
    let mut components = path.components().peekable();
    let mut file = root.to_path_buf();
    while let Some(part) = components.next() {
        file.push(part);
        let meta = match fs::symlink_metadata(&file) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            result => result.map_err(|error| fail("stat incoming image", &file, error))?,
        };
        if components.peek().is_none() {
            return fingerprint(&file, &meta).map(Some);
        }
        if !meta.is_dir() { return Ok(None); }
    }
    Ok(None)
}

#[derive(Default)]
pub(crate) struct LocalFiles {
    paths: Vec<PathBuf>,
    archived: bool,
}

impl LocalFiles {
    pub(crate) fn inspect(current: &Path, source: &Path, breenfs: bool, carry: bool,
                          installed_breenfs_image: impl FnOnce() -> Result<bool, String>) -> Result<Self, String> {
        let legacy_shared = !breenfs && match fs::read(current.join(READY)) {
            Ok(record) => record.starts_with(b"BREENIX-USERLAND 1\n"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true, // Flat ext2 root.
            Err(error) => return Err(fail("read current layout record", current, error)),
        };
        let baseline = read_baseline(current)?;
        let legacy_image = baseline.is_none() && breenfs && installed_breenfs_image()?;
        let mut paths = Vec::new();
        let mut uncopyable = false;
        for (path, current_digest) in entries(current, None, legacy_shared)? {
            if (legacy_image && breenfs_payload(&path))
                || baseline.as_ref().and_then(|files| files.get(&path)) == Some(&current_digest)
                || (!layout::SHARED.contains(&top_level(&path))
                    && incoming(source, &path, breenfs)?.as_ref() == Some(&current_digest))
            {
                continue;
            }
            uncopyable |= current_digest == "unsupported" || path.to_str().is_none();
            paths.push(path);
        }
        if !paths.is_empty() {
            // Use one serial line per path, with control characters escaped.
            let listing = paths.iter().map(|path| format!("{:?}", Path::new("/").join(path)))
                .collect::<Vec<_>>().join(", ");
            if !carry || uncopyable {
                let reason = if uncopyable { "cannot carry every local path" } else { "rerun with --carry-local" };
                for path in &paths {
                    println!("[persistent] on-box path: {:?}", Path::new("/").join(path));
                }
                println!("[persistent] on-box changes refused before copy");
                return Err(format!("on-box userland needs protection ({reason}): {listing}"));
            }
            for path in &paths {
                println!("[persistent] carrying on-box path: {:?}", Path::new("/").join(path));
            }
        }
        let archive = current.join(ARCHIVE);
        let archived = present(&archive)?;
        if archived { require_directory(&archive)?; }
        Ok(Self { paths, archived })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.paths.is_empty() && !self.archived
    }

    /// Return the roots of the changes. A newly added directory carries its
    /// whole subtree, including hard links, through the regular copier.
    fn copy_roots(&self) -> impl Iterator<Item = &Path> {
        let mut covered: Option<&Path> = None;
        self.paths.iter().map(PathBuf::as_path).filter(move |path| {
            if covered.is_some_and(|parent| path.starts_with(parent)) { return false; }
            covered = Some(path);
            true
        })
    }

    pub(super) fn apply(&self, copier: &mut Copier, current: &Path, staged: &Path, previous: &str,
                        in_draft: bool) -> Result<(), String> {
        let archive = staged.join(ARCHIVE);
        if self.archived {
            transfer(copier, &current.join(ARCHIVE), &archive, in_draft)?;
        }
        for path in self.copy_roots() {
            let actual = staged.join(path);
            // The BreenFS draft's /usr/local is a mount point. Keep changes
            // from legacy image generations in the carry archive.
            let mount_point = layout::SHARED.contains(&top_level(path))
                || in_draft && path.starts_with("usr/local");
            let target = if !mount_point && install_at(copier, staged, path)? {
                actual
            } else {
                // Keep both files when the new image owns the same path.
                let target = archive.join(previous).join(path);
                let parent = target.parent().expect("archive path has a parent");
                if !install_at(copier, staged, target.strip_prefix(staged).expect("archive belongs to staged tree"))? {
                    return Err(format!("carry archive already has {:?}", path));
                }
                copier.change("protect carry archive", &archive, || fs::set_permissions(&archive, fs::Permissions::from_mode(0o700)))?;
                copier.sync(parent)?;
                target
            };
            transfer(copier, &current.join(path), &target, in_draft)?;
            if target.starts_with(&archive) { clear_archived_setid(copier, &target)?; }
            copier.sync(target.parent().expect("carry path has a parent"))?;
        }
        Ok(())
    }
}

fn clear_archived_setid(copier: &mut Copier, path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|error| fail("stat carried path", path, error))?;
    if meta.is_symlink() { return Ok(()); }
    if meta.mode() & 0o6000 != 0 {
        let mode = fs::Permissions::from_mode(meta.mode() & 0o1777);
        copier.change("clear archived set-ID", path, || fs::set_permissions(path, mode))?;
        copier.sync(path)?;
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| fail("read carried directory", path, error))? {
            let entry = entry.map_err(|error| fail("read carried entry", path, error))?;
            clear_archived_setid(copier, &entry.path())?;
        }
    }
    Ok(())
}

fn transfer(copier: &mut Copier, source: &Path, target: &Path, in_draft: bool) -> Result<(), String> {
    if in_draft {
        copier.change("move carried path", target, || fs::rename(source, target))
    } else {
        copier.copy_tree(source, target)
    }
}

/// An existing entry or a non-directory parent makes the new image win the
/// live path. Symlinks are never followed while creating carry destinations.
fn install_at(copier: &mut Copier, root: &Path, path: &Path) -> Result<bool, String> {
    let mut components = path.components().peekable();
    let mut target = root.to_path_buf();
    while let Some(part) = components.next() {
        target.push(part);
        let leaf = components.peek().is_none();
        match fs::symlink_metadata(&target) {
            Ok(meta) if !leaf && meta.is_dir() => (),
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if leaf { return Ok(true); }
                copier.change("create carry directory", &target, || fs::DirBuilder::new().mode(0o755).create(&target))?;
                copier.sync(target.parent().expect("carry directory has a parent"))?;
            }
            Err(error) => return Err(fail("stat carry destination", &target, error)),
        }
    }
    Ok(false)
}
