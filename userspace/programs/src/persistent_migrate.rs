//! Copy the read-only persistent ext2 trees into the boot's empty BreenFS pair.
#[path = "../../../kernel/src/fs/breenfs/abi.rs"]
pub mod abi;
#[path = "persistent_migrate/core.rs"]
mod migrate;
#[cfg(target_arch = "aarch64")]
#[path = "persistent_migrate/finish.rs"]
mod finish;

use abi::{SnapshotRequest, SnapshotStatus, SNAPSHOT_CREATE, SNAPSHOT_STATUS};
use libbreenix::{
    crypto::sha256,
    error::Error,
    fs::{mount, umount2},
    layout,
    syscall::{nr, raw},
};
use migrate::{Mounts, Paths};
use std::{
    ffi::CString,
    fs::{self, File},
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::ExitCode,
};

const SOURCE_SYSTEM: &str = "/migrate/ext2-system";
const SOURCE_HOME: &str = "/migrate/ext2-home";
const SYSTEM: &str = "/migrate/system";
const HOME: &str = "/migrate/home";

struct BootMounts;

impl BootMounts {
    fn checked() -> io::Result<Self> {
        let table = fs::read_to_string("/proc/mounts")?;
        for (path, expected_kind, expected_mode) in [
            ("/", None, "ro"),
            (SOURCE_SYSTEM, Some("ext2"), "ro"),
            (SOURCE_HOME, Some("ext2"), "ro"),
            (SYSTEM, Some("breenfs"), "rw"),
            (HOME, Some("breenfs"), "rw"),
        ] {
            let found = table.lines().any(|line| {
                let mut fields = line.split_ascii_whitespace();
                fields.next();
                if fields.next() != Some(path) {
                    return false;
                }
                let kind = fields.next();
                expected_kind.is_none_or(|expected| kind == Some(expected))
                    && fields.next().and_then(|options| options.split(',').next())
                        == Some(expected_mode)
            });
            if !found {
                return Err(io::Error::other(format!(
                    "required {expected_mode} mount missing at {path}"
                )));
            }
        }
        Ok(Self)
    }

    fn remount(path: &str, root: Option<&str>, read_only: bool) -> io::Result<()> {
        let data = root
            .map(|name| CString::new(format!("root={name}")))
            .transpose()?;
        let data_ptr = data
            .as_ref()
            .map_or(std::ptr::null(), |value| value.as_ptr().cast());
        umount2(path, 0).map_err(|error| io::Error::other(format!("unmount {path}: {error}")))?;
        unsafe { mount("boot", path, "breenfs", u64::from(read_only), data_ptr) }
            .map_err(|error| io::Error::other(format!("mount {path}: {error}")))
    }
}

fn ioctl(file: &File, request: u64, arg: u64) -> io::Result<()> {
    let result = unsafe { raw::syscall3(nr::IOCTL, file.as_raw_fd() as u64, request, arg) };
    Error::from_syscall(result as i64)
        .map(|_| ())
        .map_err(|error| io::Error::other(error.to_string()))
}

impl Mounts for BootMounts {
    fn snapshot(&mut self, name: &str) -> io::Result<()> {
        let file = File::open(SYSTEM)?;
        let mut status = SnapshotStatus::default();
        ioctl(&file, SNAPSHOT_STATUS, &mut status as *mut _ as u64)?;
        if status.root_id != 1 || status.roots != 1 || status.flags != 0 {
            return Err(io::Error::other(
                "migration requires a fresh writable BreenFS system volume",
            ));
        }
        let mut request = SnapshotRequest {
            flags: 0,
            name_len: name.len() as u32,
            name: [0; 256],
        };
        request.name[..name.len()].copy_from_slice(name.as_bytes());
        ioctl(&file, SNAPSHOT_CREATE, &request as *const _ as u64)
    }

    fn switch_system(&mut self, root: Option<&str>, read_only: bool) -> io::Result<PathBuf> {
        Self::remount(SYSTEM, root, read_only)?;
        Ok(SYSTEM.into())
    }

    fn home_read_only(&mut self) -> io::Result<()> {
        Self::remount(HOME, None, true)
    }
}

fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let id = args
        .next()
        .filter(|_| args.next().is_none())
        .ok_or_else(|| io::Error::other("expected the userland generation ID"))?;
    let paths = Paths {
        source_system: Path::new(SOURCE_SYSTEM),
        source_home: Path::new(SOURCE_HOME),
        system: Path::new(SYSTEM),
        home: Path::new(HOME),
        evidence: Path::new("/run/persistent-migrate"),
    };
    migrate::run(
        &paths,
        &id,
        &mut BootMounts::checked()?,
        io::stdout().lock(),
    )
}

#[cfg(target_arch = "aarch64")]
fn prepare_shutdown_nonce() -> io::Result<String> {
    use std::io::Write;
    if std::process::id() != 1 {
        return Err(io::Error::other("migration poweroff requires PID 1"));
    }
    // PID 1 is the migrator in this boot, so there is no service supervisor
    // to prepare the authenticated shutdown or make the final syscall.
    let mut nonce = [0u8; 32];
    let count = unsafe {
        raw::syscall3(nr::GETRANDOM, nonce.as_mut_ptr() as u64, nonce.len() as u64, 0)
    };
    let count = Error::from_syscall(count as i64)
        .map_err(|error| io::Error::other(error.to_string()))?;
    if count != nonce.len() as u64 {
        return Err(io::Error::other("short shutdown nonce"));
    }
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    fs::create_dir_all("/run/breenix")?;
    let mut file = File::create("/run/breenix/shutdown-nonce")?;
    writeln!(file, "{nonce}")?;
    // Publish the nonce while /run can still commit writes.
    file.sync_all()?;
    Ok(nonce)
}

#[cfg(target_arch = "aarch64")]
fn poweroff(nonce: &str) -> io::Result<()> {
    use std::io::Write;

    {
        let mut serial = io::stdout().lock();
        writeln!(serial, "\n[init] shutdown nonce={nonce}")?;
        serial.flush()?;
    }
    let result = unsafe { raw::syscall3(142, 0xfee1dead, 672274793, 0x4321fedc) };
    Error::from_syscall(result as i64).map_err(|error| io::Error::other(error.to_string()))?;
    Err(io::Error::other("poweroff syscall returned"))
}

#[cfg(target_arch = "aarch64")]
fn main() -> ExitCode {
    finish::migrate_and_power_off(prepare_shutdown_nonce, run, poweroff)
}

#[cfg(not(target_arch = "aarch64"))]
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[migrate] failed: {}", error.to_string().escape_debug());
            ExitCode::FAILURE
        }
    }
}
