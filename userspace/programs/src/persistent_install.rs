//! `/sbin/persistent-install`: the install boot's migration (docs/metal/README.md, "Install and upgrade").
//!
//! It runs as PID 1 with the new userland on the test root as `/` and the persistent install's system partition
//! mounted at `/home`. Each top-level directory of the new root replaces the system partition's copy, except
//! `/etc` and `/root`, which are copied only when the system partition lacks them (the one-time install) and are
//! otherwise kept. `/home` (the home partition's mount point in a persistent boot), `lost+found` and the virtual
//! filesystems are not touched, nor are top-level directories the new root does not have. Ownership, modes,
//! symbolic links and hard links are kept. It syncs, prints `[persistent] install complete: ...` and then waits.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TARGET: &str = "/home";
const KEPT: [&str; 2] = ["etc", "root"];
const UNTOUCHED: [&str; 5] = ["home", "lost+found", "proc", "dev", "sys"];

#[derive(Default)]
struct Totals {
    files: u64,
    dirs: u64,
    links: u64,
    hard_links: u64,
    bytes: u64,
}

struct Copier {
    totals: Totals,
    /// The first copy of each multiply-linked file, by its source inode, so its other names become hard links.
    linked: HashMap<(u64, u64), PathBuf>,
    buffer: Vec<u8>,
}

fn fail(what: &str, path: &Path, error: std::io::Error) -> String {
    format!("{} {}: {}", what, path.display(), error)
}

fn own(path: &Path, meta: &fs::Metadata, follow: bool) -> Result<(), String> {
    let result = if follow {
        std::os::unix::fs::chown(path, Some(meta.uid()), Some(meta.gid()))
    } else {
        std::os::unix::fs::lchown(path, Some(meta.uid()), Some(meta.gid()))
    };
    result.map_err(|error| fail("chown", path, error))
}

impl Copier {
    fn copy_tree(&mut self, source: &Path, target: &Path) -> Result<(), String> {
        let meta = fs::symlink_metadata(source).map_err(|error| fail("stat", source, error))?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            let to = fs::read_link(source).map_err(|error| fail("readlink", source, error))?;
            std::os::unix::fs::symlink(&to, target).map_err(|error| fail("symlink", target, error))?;
            own(target, &meta, false)?;
            self.totals.links += 1;
        } else if kind.is_dir() {
            fs::create_dir(target).map_err(|error| fail("mkdir", target, error))?;
            let mut names: Vec<_> = fs::read_dir(source)
                .map_err(|error| fail("read directory", source, error))?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<Result<_, _>>()
                .map_err(|error| fail("read directory", source, error))?;
            names.sort();
            for name in names {
                self.copy_tree(&source.join(&name), &target.join(&name))?;
            }
            fs::set_permissions(target, fs::Permissions::from_mode(meta.mode() & 0o7777))
                .map_err(|error| fail("chmod", target, error))?;
            own(target, &meta, true)?;
            self.totals.dirs += 1;
        } else if kind.is_file() {
            let key = (meta.dev(), meta.ino());
            if meta.nlink() > 1 {
                if let Some(first) = self.linked.get(&key) {
                    fs::hard_link(first, target).map_err(|error| fail("link", target, error))?;
                    self.totals.hard_links += 1;
                    return Ok(());
                }
                self.linked.insert(key, target.to_path_buf());
            }
            let mut input = File::open(source).map_err(|error| fail("open", source, error))?;
            let mut output = File::create(target).map_err(|error| fail("create", target, error))?;
            loop {
                let count = input.read(&mut self.buffer).map_err(|error| fail("read", source, error))?;
                if count == 0 {
                    break;
                }
                output.write_all(&self.buffer[..count]).map_err(|error| fail("write", target, error))?;
                self.totals.bytes += count as u64;
            }
            drop(output);
            fs::set_permissions(target, fs::Permissions::from_mode(meta.mode() & 0o7777))
                .map_err(|error| fail("chmod", target, error))?;
            own(target, &meta, true)?;
            self.totals.files += 1;
        } else {
            // Devices, FIFOs and sockets belong to the running system, not to the userland on disk.
            println!("[persistent] left out {}: not a file, directory or link", source.display());
        }
        Ok(())
    }
}

/// Removes a tree without following symbolic links.
fn remove_tree(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|error| fail("stat", path, error))?;
    if meta.file_type().is_dir() {
        for entry in fs::read_dir(path).map_err(|error| fail("read directory", path, error))? {
            let entry = entry.map_err(|error| fail("read directory", path, error))?;
            remove_tree(&entry.path())?;
        }
        fs::remove_dir(path).map_err(|error| fail("rmdir", path, error))
    } else {
        fs::remove_file(path).map_err(|error| fail("unlink", path, error))
    }
}

fn install() -> Result<Totals, String> {
    let root_dev = fs::metadata("/").map_err(|error| fail("stat", Path::new("/"), error))?.dev();
    let target_dev = fs::metadata(TARGET).map_err(|error| fail("stat", Path::new(TARGET), error))?.dev();
    if root_dev == target_dev {
        return Err(String::from("the system partition is not mounted at /home"));
    }
    let mut names: Vec<String> = fs::read_dir("/")
        .map_err(|error| fail("read directory", Path::new("/"), error))?
        .filter_map(|entry| entry.ok().and_then(|entry| entry.file_name().into_string().ok()))
        .filter(|name| !UNTOUCHED.contains(&name.as_str()))
        .collect();
    names.sort();
    let mut copier = Copier { totals: Totals::default(), linked: HashMap::new(), buffer: vec![0u8; 64 * 1024] };
    for name in &names {
        let source = Path::new("/").join(name);
        let target = Path::new(TARGET).join(name);
        let present = fs::symlink_metadata(&target).is_ok();
        if KEPT.contains(&name.as_str()) && present {
            println!("[persistent] kept /{}", name);
            continue;
        }
        if present {
            remove_tree(&target)?;
        }
        copier.copy_tree(&source, &target)?;
        println!("[persistent] {} /{}", if present { "replaced" } else { "installed" }, name);
    }
    let home = Path::new(TARGET).join("home");
    if fs::symlink_metadata(&home).is_err() {
        fs::create_dir(&home).map_err(|error| fail("mkdir", &home, error))?;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).map_err(|error| fail("chmod", &home, error))?;
    }
    Ok(copier.totals)
}

fn main() {
    println!("[persistent] install starting: the test root's userland onto the system partition at /home");
    match install() {
        Ok(totals) => {
            // SAFETY: sync takes no arguments and only flushes filesystem state.
            unsafe { libbreenix::syscall::raw::syscall0(libbreenix::syscall::nr::SYNC) };
            println!(
                "[persistent] install complete: files={} dirs={} links={} hard-links={} bytes={}",
                totals.files, totals.dirs, totals.links, totals.hard_links, totals.bytes
            );
        }
        Err(why) => println!("[persistent] install failed: {}", why),
    }
    // PID 1 does not exit; the boot loop powers the machine off once it has read the result.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
