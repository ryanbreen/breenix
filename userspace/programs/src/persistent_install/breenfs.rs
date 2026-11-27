//! Copy the staged OS tree into an independent root on the BreenFS system volume.
use super::{fail, layout, local, own, read_fixtures, recovered_slots, require_directory, valid_id,
            Copier, Totals, READY};
use crate::breenfs_abi::{SnapshotNext, SnapshotRename, SnapshotRequest, SNAPSHOT_CREATE, SNAPSHOT_DELETE,
          SNAPSHOT_EMPTY, SNAPSHOT_NEXT, SNAPSHOT_READ_ONLY, SNAPSHOT_RENAME};
use libbreenix::{
    error::Error,
    fs::{mount, umount2},
    syscall::{nr, raw},
};
use std::{
    collections::HashMap,
    ffi::CString,
    fs::{self, File},
    os::{fd::AsRawFd, unix::fs::{MetadataExt, PermissionsExt}},
    path::Path,
};

const SYSTEM: &str = "/migrate/system";
const HOME: &str = "/migrate/home";

pub(super) struct Upgrade;

impl Upgrade {
    pub(super) fn detect() -> Result<Option<Self>, String> {
        let mounts = fs::read_to_string("/proc/mounts").map_err(|error| error.to_string())?;
        let required = [("/", "ext2", "ro"), (SYSTEM, "breenfs", "rw"), (HOME, "breenfs", "rw")];
        let mut found = [false; 3];
        for line in mounts.lines() {
            let mut fields = line.split_ascii_whitespace().skip(1);
            let (Some(path), Some(kind), Some(flags)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            if let Some(index) = required.iter().position(|row| row.0 == path) {
                let (_, expected_kind, expected_mode) = required[index];
                found[index] = kind == expected_kind && flags.split(',').next() == Some(expected_mode);
                // A present provisioning mount must have the requested format and mode.
                if index != 0 && !found[index] {
                    return Err(format!("unexpected BreenFS upgrade mount at {path}"));
                }
            }
        }
        match found {
            [_, false, false] => Ok(None),
            [true, true, true] => Ok(Some(Self)),
            _ => Err("incomplete BreenFS upgrade mount set".into()),
        }
    }

    pub(super) fn install(self, source: &Path) -> Result<Totals, String> {
        println!("[persistent] install starting: the test root's userland onto BreenFS");
        let id = std::env::args().nth(1).filter(|id| valid_id(id))
            .ok_or("upgrade intent names no valid userland generation")?;
        let (references, carry) = recovered_slots(source)?;
        let slots = references.each_ref().map(String::as_str);
        if slots[2] != id || matches!(slots[0], "empty" | "flat") {
            return Err("recovered slots do not name the current generation and upgrade candidate".into());
        }
        let published = format!("gen-{id}");
        let draft = format!("{id}.new");
        let current = format!("gen-{}", slots[0]);
        remount(Some(&current), true)?;
        let local_files = local::LocalFiles::inspect(Path::new(SYSTEM), source, true, carry)?;
        remount(None, false)?;
        {
            let system = open(SYSTEM)?;
            let mut roots = SnapshotNext { id: 0, root: request("", 0) };
            let mut retained = [false, slots[1] == "empty"];
            let mut remove = Vec::new();
            loop {
                ioctl(&system, SNAPSHOT_NEXT, &mut roots)?;
                if roots.id == 0 { break; }
                let name = std::str::from_utf8(&roots.root.name[..roots.root.name_len as usize])
                    .map_err(|error| error.to_string())?;
                if name == published {
                    return Err("the upgrade candidate is already published".into());
                }
                if let Some(generation) = name.strip_prefix("gen-") {
                    for (found, slot) in retained.iter_mut().zip(&slots[..2]) {
                        *found |= generation == *slot;
                    }
                    if roots.root.flags == 0 && is_metal_generation(generation) && !slots.contains(&generation) {
                        remove.push(name.to_owned());
                    }
                } else if roots.root.flags == 0 && (name == draft
                    || name.strip_suffix(".new").is_some_and(is_metal_generation)) {
                    remove.push(name.to_owned());
                }
            }
            if retained.contains(&false) {
                return Err("a retained slot has no BreenFS generation".into());
            }
            let home = open(HOME)?;
            let mut snapshot = request(&format!("pre-upgrade-{id}"), SNAPSHOT_READ_ONLY);
            ioctl(&home, SNAPSHOT_CREATE, &mut snapshot)?;
            for name in remove {
                ioctl(&system, SNAPSHOT_DELETE, &mut request(&name, 0))?;
                println!("[persistent] reclaimed: {name}");
            }
            if local_files.is_empty() {
                ioctl(&system, SNAPSHOT_CREATE, &mut request(&draft, SNAPSHOT_EMPTY))?;
            }
        }
        let staged = Path::new(SYSTEM);
        let scratch = if local_files.is_empty() {
            remount(Some(&draft), false)?;
            None
        } else {
            // Clone CURRENT and move its contents aside inside the clone. The
            // old tree stays available for rollback and no scratch writes go to home.
            remount(Some(&current), false)?;
            ioctl(&open(SYSTEM)?, SNAPSHOT_CREATE, &mut request(&draft, 0))?;
            remount(Some(&draft), false)?;
            Some(move_old_tree(staged, &id)?)
        };
        let totals = copy_generation(source, staged, &id, &local_files, slots[0], scratch.as_deref())?;
        remount(None, false)?;
        ioctl(&open(SYSTEM)?, SNAPSHOT_RENAME, &mut SnapshotRename {
            from: request(&draft, 0),
            to: request(&published, 0),
        })?;
        println!("[persistent] layout set-ID check passed");
        Ok(totals)
    }
}

fn is_metal_generation(name: &str) -> bool {
    let Some(id) = name.strip_suffix("-metal") else { return false; };
    id.len() == 16 && id.as_bytes()[8] == b'T' && id.as_bytes()[15] == b'Z'
        && id.as_bytes()[..8].iter().chain(&id.as_bytes()[9..15]).all(u8::is_ascii_digit)
}

fn move_old_tree(staged: &Path, id: &str) -> Result<std::path::PathBuf, String> {
    let name = format!(".breenix-carry-{id}");
    let scratch = staged.join(&name);
    fs::create_dir(&scratch).map_err(|error| fail("create carry scratch", &scratch, error))?;
    let entries = fs::read_dir(staged).and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
        .map_err(|error| fail("list snapshot", staged, error))?;
    for entry in entries {
        if entry.file_name() != name.as_str() {
            fs::rename(entry.path(), scratch.join(entry.file_name()))
                .map_err(|error| fail("move old path within draft", &entry.path(), error))?;
        }
    }
    Ok(scratch)
}

fn copy_generation(source: &Path, staged: &Path, id: &str, local_files: &local::LocalFiles,
                   previous: &str, scratch: Option<&Path>) -> Result<Totals, String> {
    let fixtures = read_fixtures(source)?;
    let mut copier = Copier {
        totals: Totals::default(), skip: Default::default(), linked: HashMap::new(),
        buffer: vec![0; 64 * 1024], checkpoint: &mut || Ok(()),
    };
    for name in ["bin", "sbin", "usr"] {
        require_directory(&source.join(name))?;
        copier.copy_tree(&source.join(name), &staged.join(name))?;
    }
    let factory = staged.join("usr/share/factory");
    super::drop_test_account(&mut copier, &factory.join("etc"), &fixtures)?;
    copier.write_file(&factory.join("generation"), id, 0o644)?;
    for name in ["root", "home", "dev", "proc", "sys"].into_iter().chain(layout::SHARED) {
        let point = staged.join(name);
        fs::create_dir(&point).map_err(|error| fail("create mount point", &point, error))?;
        let mode = match name {
            "root" => 0o700,
            "tmp" => 0o1777,
            _ => 0o755,
        };
        fs::set_permissions(&point, fs::Permissions::from_mode(mode))
            .map_err(|error| fail("chmod mount point", &point, error))?;
    }
    let root_meta = fs::symlink_metadata(source).map_err(|error| fail("stat", source, error))?;
    own(staged, &root_meta, true).map_err(|error| fail("chown", staged, error))?;
    fs::set_permissions(staged, fs::Permissions::from_mode(root_meta.mode() & 0o7777))
        .map_err(|error| fail("chmod", staged, error))?;
    local::record(&mut copier, staged, scratch.and_then(|path| path.strip_prefix(staged).ok()))?;
    if let Some(scratch) = scratch {
        local_files.apply(&mut copier, scratch, staged, previous, true)?;
        fs::remove_dir_all(scratch).map_err(|error| fail("remove old draft paths", scratch, error))?;
    }
    layout::check_setid(staged, false)?;
    copier.write_file(&staged.join(READY), &layout::record(id), 0o644)?;
    copier.sync(staged)?;
    Ok(copier.totals)
}

fn open(path: &str) -> Result<File, String> {
    File::open(path).map_err(|error| fail("open", Path::new(path), error))
}

fn request(name: &str, flags: u32) -> SnapshotRequest {
    let mut request = SnapshotRequest { flags, name_len: name.len() as u32, name: [0; 256] };
    request.name[..name.len()].copy_from_slice(name.as_bytes());
    request
}

fn ioctl<T>(file: &File, operation: u64, arg: &mut T) -> Result<(), String> {
    let result = unsafe { raw::syscall3(nr::IOCTL, file.as_raw_fd() as u64, operation, arg as *mut T as u64) };
    Error::from_syscall(result as i64).map(|_| ()).map_err(|error| error.to_string())
}

fn remount(name: Option<&str>, read_only: bool) -> Result<(), String> {
    let data = name.map(|name| CString::new(format!("root={name}"))).transpose()
        .map_err(|error| error.to_string())?;
    let data = data.as_ref().map_or(std::ptr::null(), |data| data.as_ptr().cast());
    umount2(SYSTEM, 0).map_err(|error| format!("unmount {SYSTEM}: {error}"))?;
    unsafe { mount("boot", SYSTEM, "breenfs", u64::from(read_only), data) }.map_err(|error| format!("mount {SYSTEM}: {error}"))
}
