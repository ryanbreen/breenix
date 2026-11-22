#![forbid(unsafe_code)]

//! Deterministic common archives per Solaris ar(3HEAD), with ELF gABI §5 symbols.

use bobj::ar::{self, Archive, IndexEntry};
use bobj::elf::{Elf, ET_REL, SHN_UNDEF, STB_GLOBAL, STB_WEAK};
use std::borrow::Cow;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{self, ExitCode};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Member<'a> {
    name: &'a [u8],
    bytes: Cow<'a, [u8]>,
}

fn encode(path: &Path, members: &[Member<'_>]) -> Result<Vec<u8>, String> {
    let mut index = Vec::new();
    for (member, entry) in members.iter().enumerate() {
        if !entry.bytes.starts_with(b"\x7fELF") {
            continue;
        }
        let result = (|| {
            let file = Elf::parse(&entry.bytes)?;
            if file.header().file_type != ET_REL {
                return Err(bobj::Error {
                    offset: 16,
                    message: "expected an ELF relocatable archive member",
                });
            }
            if let Some(symbols) = file.static_symbols()? {
                for i in 1..symbols.len() {
                    let symbol = symbols.get(i)?;
                    if matches!(symbol.binding, STB_GLOBAL | STB_WEAK)
                        && symbol.section != SHN_UNDEF
                        && !symbol.name.is_empty()
                    {
                        index.push(IndexEntry {
                            name: symbol.name,
                            member,
                        });
                    }
                }
            }
            Ok(())
        })();
        result.map_err(|error| {
            format!(
                "{}({}): {error}",
                path.display(),
                String::from_utf8_lossy(entry.name)
            )
        })?;
    }
    let contents: Vec<_> = members
        .iter()
        .map(|entry| (entry.name, entry.bytes.as_ref()))
        .collect();
    ar::write(&contents, &index).map_err(|error| format!("{}: {error}", path.display()))
}

fn publish(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let directory = loop {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".bar-{}-{sequence}", process::id()));
        match DirBuilder::new().mode(0o700).create(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    };
    let temporary = directory.join("archive");
    let result = (|| {
        // New archives use the caller's umask. Replacements retain their mode.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o666)
            .open(&temporary)?;
        file.write_all(bytes)?;
        if let Some(mode) = mode {
            file.set_permissions(Permissions::from_mode(mode))?;
        }
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    let _ = fs::remove_dir(directory);
    result
}

fn update(path: &Path, inputs: &[OsString], replace: bool) -> Result<(), String> {
    let at_archive = |error: io::Error| format!("{}: {error}", path.display());
    let (bytes, mode) = match fs::File::open(path) {
        Ok(mut file) => {
            use std::io::Read;
            let mode = file.metadata().map_err(at_archive)?.permissions().mode() & 0o777;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(at_archive)?;
            (Some(bytes), Some(mode))
        }
        Err(error) if replace && error.kind() == io::ErrorKind::NotFound => (None, None),
        Err(error) => return Err(at_archive(error)),
    };
    let mut members = Vec::new();
    if let Some(bytes) = &bytes {
        // The old directory is replaced from the surviving members, even when
        // its offsets are corrupt. Member framing still has to be valid.
        let original = Archive::members_for_reindex(bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        members.extend(original.into_iter().map(|entry| Member {
            name: entry.name,
            bytes: Cow::Borrowed(entry.bytes),
        }));
    }
    for input in inputs {
        let input = Path::new(input);
        let at_input = |error: &dyn std::fmt::Display| format!("{}: {error}", input.display());
        let name = input
            .as_os_str()
            .as_encoded_bytes()
            .rsplit(|&byte| byte == b'/')
            .next()
            .filter(|name| !name.is_empty() && !name.contains(&b'\r') && !name.contains(&b'\n'))
            .ok_or_else(|| at_input(&"invalid member name"))?;
        let entry = Member {
            name,
            bytes: Cow::Owned(fs::read(input).map_err(|error| at_input(&error))?),
        };
        if let Some(existing) = members.iter_mut().find(|existing| existing.name == name) {
            *existing = entry;
        } else {
            members.push(entry);
        }
    }
    publish(path, &encode(path, &members)?, mode).map_err(at_archive)
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args: Vec<_> = args.into_iter().collect();
    if let [operation, archive, inputs @ ..] = args.as_slice() {
        let operation = operation.as_encoded_bytes();
        let operation = operation.strip_prefix(b"-").unwrap_or(operation);
        let replace = matches!(operation, b"r" | b"rc" | b"rs" | b"rcs");
        if replace || operation == b"s" && inputs.is_empty() {
            return match update(Path::new(archive), inputs, replace) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bar: {error}");
                    ExitCode::FAILURE
                }
            };
        }
    }
    eprintln!("usage: bar r[c][s] archive [file ...]\n       bar s archive");
    ExitCode::from(2)
}
