//! The live writer: the booted root's BreenFS ioctls and the staging mount.
use super::session::{valid_root, Places, Roots, Session, Writer, PROTOCOL};
use super::*;
use crate::{
    breenfs::request,
    breenfs_abi::{
        RootEpoch, SnapshotNext, SnapshotRename, ROOT_EPOCH, ROOT_FREEZE, ROOT_THAW,
        SNAPSHOT_CREATE, SNAPSHOT_DELETE, SNAPSHOT_NEXT, SNAPSHOT_RENAME,
    },
};
use libbreenix::{
    errno::Errno,
    error::Error,
    fs::{fstatfs, mount, umount2},
    syscall::{nr, raw},
    types::Fd,
};
use std::{ffi::CString, fs, os::fd::AsRawFd, thread, time::Duration};

const STAGE: &str = "/run/breenix/paired/stage";

fn ctl<T>(file: &File, op: u64, arg: &mut T) -> Result<(), Error> {
    let value =
        unsafe { raw::syscall3(nr::IOCTL, file.as_raw_fd() as u64, op, arg as *mut T as u64) };
    Error::from_syscall(value as i64).map(|_| ())
}

fn mount_list() -> Result<String, String> {
    fs::read_to_string("/proc/mounts").map_err(io_error)
}

struct Live {
    root: File,
}

impl Writer for Live {
    fn roots(&self) -> Result<Roots, String> {
        let mut next = SnapshotNext {
            id: 0,
            root: request("", 0),
        };
        let mut roots = BTreeMap::new();
        loop {
            ctl(&self.root, SNAPSHOT_NEXT, &mut next).map_err(|err| err.to_string())?;
            if next.id == 0 {
                return Ok(roots);
            }
            let name = std::str::from_utf8(&next.root.name[..next.root.name_len as usize])
                .map_err(|err| err.to_string())?;
            roots.insert(name.into(), (next.id, next.root.flags));
        }
    }

    fn create(&self, name: &str) -> Result<(), String> {
        ctl(&self.root, SNAPSHOT_CREATE, &mut request(name, 0)).map_err(|err| err.to_string())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        ctl(&self.root, SNAPSHOT_DELETE, &mut request(name, 0)).map_err(|err| err.to_string())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let mut names = SnapshotRename {
            from: request(from, 0),
            to: request(to, 0),
        };
        ctl(&self.root, SNAPSHOT_RENAME, &mut names).map_err(|err| err.to_string())
    }

    fn epoch(&self) -> Result<u64, String> {
        let mut value = RootEpoch::default();
        ctl(&self.root, ROOT_EPOCH, &mut value).map_err(|err| err.to_string())?;
        Ok(value.epoch)
    }

    fn freeze(&self) -> Result<u64, String> {
        let mut value = RootEpoch::default();
        let mut retries = 2;
        loop {
            match ctl(&self.root, ROOT_FREEZE, &mut value) {
                Ok(()) => return Ok(value.epoch),
                Err(Error::Os(Errno::EBUSY)) if retries != 0 => {
                    retries -= 1;
                    thread::sleep(Duration::from_secs(1));
                }
                Err(err) => {
                    return Err(format!(
                        "root freeze: {err} (writable mappings {}, dirty files {})",
                        value.writable_mappings, value.dirty_files
                    ))
                }
            }
        }
    }

    fn thaw(&self) -> Result<(), String> {
        ctl(&self.root, ROOT_THAW, &mut RootEpoch::default()).map_err(|err| err.to_string())
    }

    fn attach(&self, name: &str) -> Result<(), String> {
        let data = CString::new(format!("root={name}")).map_err(|err| err.to_string())?;
        unsafe { mount("root-volume", STAGE, "breenfs", 0, data.as_ptr().cast()) }
            .map_err(|err| err.to_string())
    }

    fn detach(&self) -> Result<(), String> {
        umount2(STAGE, 0).map_err(|err| err.to_string())
    }

    fn stage(&self) -> Result<PathBuf, String> {
        if !mount_list()?
            .lines()
            .any(|line| line.split_ascii_whitespace().nth(1) == Some(STAGE))
        {
            return Err("no staging root is mounted".into());
        }
        Ok(PathBuf::from(STAGE))
    }

    fn available(&self) -> Result<u64, String> {
        let stats =
            fstatfs(Fd::from_raw(self.root.as_raw_fd() as u64)).map_err(|err| err.to_string())?;
        Ok(stats.f_bavail.saturating_mul(stats.f_bsize as u64))
    }
}

fn booted_root() -> Result<String, String> {
    mount_list()?
        .lines()
        .find_map(|line| {
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            match fields.as_slice() {
                [source, "/", "breenfs", flags, ..]
                    if flags.split(',').next() == Some("rw") && valid_root(source) =>
                {
                    Some(source.to_string())
                }
                _ => None,
            }
        })
        .ok_or_else(|| "paired deploy requires a named writable BreenFS root".into())
}

pub(super) fn run(args: Vec<String>) -> Result<(), String> {
    if args == ["protocol"] {
        println!("{PROTOCOL}");
        return Ok(());
    }
    if args.is_empty() {
        return Err("missing paired subcommand".into());
    }
    let live = Live {
        root: Dir::open(Path::new("/")).map_err(io_error)?.file,
    };
    let places = Places {
        root: Path::new("/"),
        run: Path::new("/run"),
        home: Path::new("/root"),
        var: Path::new("/var"),
        opt: Path::new("/opt"),
    };
    let tool = PathBuf::from(std::env::args_os().next().ok_or("missing tool path")?);
    let session = Session::open(&places, booted_root()?, tool, &live)?;
    session.run(&args, &mut io::stdout())
}
