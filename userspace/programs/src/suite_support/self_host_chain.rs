//! Artifact snapshots for the chained E25 builds.
use crate::sha256::Sha256;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::time::SystemTime;

pub const PROFILE: &str = "x86_64-breenix/selfhost";
pub const TOOLS: [&str; 6] = ["bas", "bld", "bar", "bnm", "bcc", "bmake"];

#[derive(Clone)]
pub struct Artifact {
    pub sha256: String,
    pub modified: SystemTime,
}

pub type Snapshot = BTreeMap<String, Artifact>;
pub type Digests = BTreeMap<String, String>;

pub fn snapshot(target: &Path) -> io::Result<Snapshot> {
    let mut files = BTreeMap::new();
    collect(target, target, &mut files)?;
    Ok(files)
}

fn collect(target: &Path, dir: &Path, files: &mut Snapshot) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect(target, &path, files)?;
        } else if metadata.is_file() {
            let name = path
                .strip_prefix(target)
                .expect("entry is under the target root");
            let name = name.to_str().ok_or_else(|| {
                io::Error::other(format!("non-UTF-8 Cargo output path: {}", path.display()))
            })?;
            let mut file = File::open(&path)?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 32 * 1024];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            let sha256 = hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            files.insert(
                name.to_owned(),
                Artifact {
                    sha256,
                    modified: metadata.modified()?,
                },
            );
        } else {
            return Err(io::Error::other(format!(
                "unexpected Cargo output: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

pub fn verify(recorded: &Digests, current: &Snapshot) -> Result<(), String> {
    for (path, digest) in recorded {
        let file = current
            .get(path)
            .ok_or_else(|| format!("missing Cargo output {path}"))?;
        if file.sha256 != *digest {
            return Err(format!("SHA-256 changed for {path}"));
        }
    }
    if let Some(path) = current.keys().find(|path| !recorded.contains_key(*path)) {
        return Err(format!("unexpected Cargo output {path}"));
    }
    Ok(())
}

pub fn verify_noop(before: &Snapshot, after: &Snapshot) -> Result<(), String> {
    let digests = before
        .iter()
        .map(|(path, file)| (path.clone(), file.sha256.clone()))
        .collect();
    verify(&digests, after)?;
    for (path, file) in before {
        if after[path].modified != file.modified && !top_level_tool_dep_info(path) {
            return Err(format!("mtime changed for {path}"));
        }
    }
    Ok(())
}

fn top_level_tool_dep_info(path: &str) -> bool {
    // Ruling 137: x86_64-breenix/selfhost/{,lib}<tool>.d for the six tools.
    let Some((PROFILE, file)) = path.rsplit_once('/') else {
        return false;
    };
    file.strip_suffix(".d")
        .is_some_and(|stem| TOOLS.contains(&stem.strip_prefix("lib").unwrap_or(stem)))
}
