use crate::{normal_relative, parent, Result};
use glob::{MatchOptions, Pattern};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Dir(u32),
    File {
        size: u64,
        mode: u32,
        digest: String,
    },
    Other,
}

pub type Tree = BTreeMap<String, Entry>;

#[derive(Default)]
pub struct Excludes(Vec<(bool, Pattern)>);

impl Excludes {
    pub fn new(patterns: &[String]) -> Result<Self> {
        let mut compiled = Vec::new();
        for text in patterns {
            let text = text.trim_end_matches('/');
            let pattern = text.strip_prefix('/').unwrap_or(text);
            if pattern.is_empty() {
                return Err("empty exclude glob".into());
            }
            compiled.push((text.contains('/'), Pattern::new(pattern)?));
        }
        Ok(Self(compiled))
    }

    fn matches(&self, path: &str) -> bool {
        let mut current = path;
        while !current.is_empty() {
            let basename = current.rsplit('/').next().unwrap();
            if self.0.iter().any(|(anchored, pattern)| {
                pattern.matches_with(
                    if *anchored { current } else { basename },
                    MatchOptions {
                        require_literal_separator: true,
                        ..MatchOptions::new()
                    },
                )
            }) {
                return true;
            }
            current = parent(current);
        }
        false
    }
}

pub fn scan(root: &Path, excludes: &Excludes) -> Result<Tree> {
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() {
        return Err(format!("source must be a real directory: {root:?}").into());
    }
    let mut entries = Tree::new();
    scan_entry(root, "", excludes, &mut entries)?;
    Ok(entries)
}

fn scan_entry(path: &Path, relative: &str, excludes: &Excludes, entries: &mut Tree) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let mode = metadata.permissions().mode() & 0o7777;
    if mode > 0o777 {
        return Err(format!("source has special mode bits: {path:?}").into());
    }
    let entry = if metadata.is_dir() {
        for child in fs::read_dir(path)? {
            let child = child?;
            let name = child.file_name();
            let name = name.to_str().ok_or("source file name is not UTF-8")?;
            let name = if relative.is_empty() {
                name.to_owned()
            } else {
                format!("{relative}/{name}")
            };
            if excludes.matches(&name) {
                continue;
            }
            if !normal_relative(&name) {
                return Err(format!("unsupported source path {name:?}").into());
            }
            scan_entry(&child.path(), &name, excludes, entries)?;
        }
        Entry::Dir(mode)
    } else if metadata.is_file() {
        let mut file = File::open(path)?;
        let mut hash = Sha256::new();
        let mut bytes = [0; 32 * 1024];
        let mut size = 0;
        loop {
            let count = file.read(&mut bytes)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            hash.update(&bytes[..count]);
        }
        if size != metadata.len() {
            return Err(format!("source changed while reading {path:?}").into());
        }
        Entry::File {
            size,
            mode,
            digest: format!("{:x}", hash.finalize()),
        }
    } else {
        return Err(format!("unsupported source file type: {path:?}").into());
    };
    entries.insert(relative.into(), entry);
    Ok(())
}

#[derive(Debug)]
pub enum Action {
    MakeDir(String),
    Upload(String),
    FileMode(String),
    DirMode(String),
    RemoveFile(String),
    RemoveDir(String),
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Self::MakeDir(_) => "MKDIR",
            Self::Upload(_) => "UPLOAD",
            Self::FileMode(_) | Self::DirMode(_) => "CHMOD",
            Self::RemoveFile(_) => "DELETE",
            Self::RemoveDir(_) => "RMDIR",
        }
    }

    pub fn path(&self) -> &str {
        match self {
            Self::MakeDir(path)
            | Self::Upload(path)
            | Self::FileMode(path)
            | Self::DirMode(path)
            | Self::RemoveFile(path)
            | Self::RemoveDir(path) => path,
        }
    }
}

pub fn plan(local: &Tree, remote: &Tree, excludes: &Excludes, delete: bool) -> Result<Vec<Action>> {
    let mut protected = BTreeSet::new();
    for path in remote.keys().filter(|path| excludes.matches(path)) {
        let mut current = path.as_str();
        while !current.is_empty() && protected.insert(current) {
            current = parent(current);
        }
    }
    let mut replacements = BTreeSet::new();
    let mut mkdir = Vec::new();
    let mut files = Vec::new();
    let mut modes = Vec::new();

    for (path, source) in local.iter().filter(|(path, _)| !excludes.matches(path)) {
        let mut target = remote.get(path);
        if matches!(target, Some(Entry::Other)) {
            return Err(
                format!("destination contains a symlink or special file at {path:?}").into(),
            );
        }
        if matches!(
            (source, target),
            (Entry::Dir(_), Some(Entry::File { .. })) | (Entry::File { .. }, Some(Entry::Dir(_)))
        ) {
            if !delete || protected.contains(path.as_str()) {
                return Err(format!("cannot replace a different file type at {path:?}").into());
            }
            replacements.insert(path.as_str());
            let prefix = format!("{path}/");
            replacements.extend(
                remote
                    .range(prefix.clone()..)
                    .map(|(child, _)| child.as_str())
                    .take_while(|child| child.starts_with(&prefix)),
            );
            target = None;
        }
        match (source, target) {
            (Entry::Dir(_), None) => {
                mkdir.push(Action::MakeDir(path.clone()));
                modes.push(Action::DirMode(path.clone()));
            }
            (Entry::Dir(mode), Some(Entry::Dir(old))) if mode != old => {
                modes.push(Action::DirMode(path.clone()));
            }
            (
                Entry::File { size, digest, .. },
                Some(Entry::File {
                    size: old_size,
                    digest: old_digest,
                    ..
                }),
            ) if size == old_size && digest == old_digest => {
                if Some(source) != target {
                    files.push(Action::FileMode(path.clone()));
                }
            }
            (Entry::File { .. }, _) => files.push(Action::Upload(path.clone())),
            _ => {}
        }
    }

    let mut extra = Vec::new();
    if delete {
        for (path, entry) in remote {
            if path.is_empty() || protected.contains(path.as_str()) {
                continue;
            }
            if matches!(entry, Entry::Other) {
                return Err(
                    format!("destination contains a symlink or special file at {path:?}").into(),
                );
            }
            if !local.contains_key(path) && !replacements.contains(path.as_str()) {
                extra.push(remove(path, entry));
            }
        }
    }
    let mut actions: Vec<_> = replacements
        .into_iter()
        .map(|path| remove(path, &remote[path]))
        .collect();
    deepest_first(&mut actions);
    deepest_first(&mut extra);
    deepest_first(&mut modes);
    actions.extend(mkdir);
    actions.extend(files);
    actions.extend(extra);
    actions.extend(modes);
    Ok(actions)
}

fn remove(path: &str, entry: &Entry) -> Action {
    if matches!(entry, Entry::Dir(_)) {
        Action::RemoveDir(path.into())
    } else {
        Action::RemoveFile(path.into())
    }
}

fn deepest_first(actions: &mut [Action]) {
    actions.sort_by(|a, b| {
        let key =
            |action: &Action| Reverse(action.path().split('/').filter(|p| !p.is_empty()).count());
        key(a).cmp(&key(b)).then_with(|| a.path().cmp(b.path()))
    });
}
