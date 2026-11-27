use crate::{json::Json, Fields};
use std::collections::BTreeMap;
use std::fs::{self, File, FileTimes, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FIXTURE_BYTES: usize = 16 * 1024 * 1024;

pub struct Fixture(BTreeMap<PathBuf, Entry>);

enum Entry {
    Directory(Attributes),
    File(Attributes, Vec<u8>, usize),
    Symlink(String),
}

struct Attributes {
    mode: u32,
    mtime: Option<SystemTime>,
}

impl Attributes {
    fn parse(fields: &mut Fields, default_mode: u32) -> Result<Self, String> {
        let mode = match fields.string("mode")? {
            Some(octal) => u32::from_str_radix(&octal, 8)
                .ok()
                .filter(|&mode| mode <= 0o7777)
                .ok_or("invalid octal mode")?,
            None => default_mode,
        };
        let mtime = fields
            .integer("mtime")?
            .map(|seconds| {
                UNIX_EPOCH
                    .checked_add(Duration::from_secs(seconds))
                    .ok_or("mtime out of range")
            })
            .transpose()?;
        Ok(Self { mode, mtime })
    }

    fn apply(&self, file: &File) -> io::Result<()> {
        file.set_permissions(Permissions::from_mode(self.mode))?;
        if let Some(time) = self.mtime {
            file.set_times(FileTimes::new().set_modified(time))?;
        }
        Ok(())
    }
}

impl Fixture {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut root = Fields::new(Json::parse(text)?)?;
        let mut entries = BTreeMap::new();
        let mut bytes_left = FIXTURE_BYTES;
        for value in root.array("entries")? {
            let mut fields = Fields::new(value)?;
            let name = fields.required_string("path")?;
            if name.split('/').any(|part| matches!(part, "" | "." | "..")) {
                return Err(format!("invalid fixture path {name:?}"));
            }
            let entry = match fields.required_string("kind")?.as_str() {
                "directory" => Entry::Directory(Attributes::parse(&mut fields, 0o755)?),
                "symlink" => Entry::Symlink(fields.required_string("target")?),
                "file" => {
                    let bytes = match (fields.0.remove("text"), fields.string("hex")?) {
                        (Some(Json::String(text)), None) => text.into_bytes(),
                        (None, Some(hex)) => decode_hex(&hex)?,
                        (None, None) => Vec::new(),
                        _ => return Err(format!("{name}: expected one of text or hex")),
                    };
                    let repeat = usize::try_from(fields.integer("repeat")?.unwrap_or(1))
                        .map_err(|_| "fixture repeat out of range")?;
                    let size = bytes
                        .len()
                        .checked_mul(repeat)
                        .ok_or("fixture file too large")?;
                    bytes_left = bytes_left
                        .checked_sub(size)
                        .ok_or("fixture exceeds 16 MiB")?;
                    Entry::File(Attributes::parse(&mut fields, 0o644)?, bytes, repeat)
                }
                kind => return Err(format!("{name}: unknown fixture kind {kind:?}")),
            };
            fields.finish()?;
            if entries.insert(PathBuf::from(&name), entry).is_some() {
                return Err(format!("duplicate fixture path {name:?}"));
            }
        }
        root.finish()?;
        for path in entries.keys() {
            let parent = path
                .parent()
                .expect("fixture paths are relative and nonempty");
            if !parent.as_os_str().is_empty()
                && !matches!(entries.get(parent), Some(Entry::Directory(_)))
            {
                return Err(format!(
                    "{} must have an explicit directory parent",
                    path.display()
                ));
            }
        }
        Ok(Self(entries))
    }

    /// Materialize into an empty directory owned by the caller.
    pub fn materialize(&self, root: &Path) -> io::Result<()> {
        if !fs::symlink_metadata(root)?.is_dir() || fs::read_dir(root)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture root must be an empty directory",
            ));
        }
        for (path, entry) in &self.0 {
            let path = root.join(path);
            match entry {
                Entry::Directory(_) => fs::create_dir(&path)?,
                Entry::Symlink(target) => symlink(target, path)?,
                Entry::File(attrs, bytes, repeat) => {
                    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
                    if !bytes.is_empty() {
                        // Make one write for common generators such as a repeated byte.
                        file.write_all(&bytes.repeat(*repeat))?;
                    }
                    attrs.apply(&file)?;
                }
            }
        }
        // Directory permissions and mtimes are final only after creating children.
        for (path, entry) in self.0.iter().rev() {
            if let Entry::Directory(attrs) = entry {
                attrs.apply(&File::open(root.join(path))?)?;
            }
        }
        Ok(())
    }
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid fixture hex".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}
