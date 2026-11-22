use super::{publish, Failure, PublishIo};
use libbreenix::{fs, io as sysio, signal, Fd};
use std::ffi::{c_char, CString};
use std::fs::{File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{
    ffi::OsStrExt,
    fs::{OpenOptionsExt, PermissionsExt},
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

extern "C" {
    fn openat(dirfd: i32, path: *const c_char, flags: i32, mode: u32) -> i32;
    fn renameat(oldfd: i32, old: *const c_char, newfd: i32, new: *const c_char) -> i32;
    fn unlinkat(dirfd: i32, path: *const c_char, flags: i32) -> i32;
}

static CANCELLED: AtomicBool = AtomicBool::new(false);

extern "C" fn cancel(_: i32) {
    CANCELLED.store(true, Ordering::Relaxed);
}

fn check_cancelled() -> io::Result<()> {
    if CANCELLED.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "session interrupted",
        ));
    }
    Ok(())
}

struct Guest {
    parent: PathBuf,
    dest: CString,
    temp: CString,
    dir: Option<File>,
}

impl Guest {
    fn dir_fd(&self) -> i32 {
        self.dir.as_ref().expect("directory open").as_raw_fd()
    }
}

fn status(result: i32) -> io::Result<i32> {
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(result)
}

impl PublishIo for Guest {
    type File = File;

    fn open_dir(&mut self) -> io::Result<()> {
        self.dir = Some(
            OpenOptions::new()
                .read(true)
                .custom_flags(fs::O_DIRECTORY as i32)
                .open(&self.parent)?,
        );
        Ok(())
    }

    fn create_temp(&mut self) -> io::Result<File> {
        check_cancelled()?;
        let flags = fs::O_WRONLY | fs::O_CREAT | fs::O_EXCL | sysio::status_flags::O_CLOEXEC as u32;
        let fd = status(unsafe { openat(self.dir_fd(), self.temp.as_ptr(), flags as i32, 0o600) })?;
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn read_stdin(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        check_cancelled()?;
        sysio::read(Fd::from_raw(0), buf).map_err(|error| io::Error::other(error.to_string()))
    }

    fn write_temp(&mut self, file: &mut File, buf: &[u8]) -> io::Result<usize> {
        check_cancelled()?;
        file.write(buf)
    }

    fn progress(&mut self, received: u64) -> io::Result<()> {
        writeln!(io::stderr().lock(), "received {received}")
    }

    fn chmod(&mut self, file: &File, mode: u32) -> io::Result<()> {
        file.set_permissions(Permissions::from_mode(mode))
    }

    fn sync_file(&mut self, file: &File) -> io::Result<()> {
        file.sync_all()
    }

    fn rename(&mut self) -> io::Result<()> {
        // The SSH exec relay closes its output pipe readers before closing
        // stdin on a transport error. This distinguishes that EOF from a
        // normal channel EOF after a complete payload.
        use sysio::poll_events::{POLLERR, POLLHUP, POLLNVAL, POLLOUT};
        let mut pipes = [sysio::PollFd::new(Fd::from_raw(2), POLLOUT)];
        sysio::poll(&mut pipes, 0).map_err(|error| io::Error::other(error.to_string()))?;
        if pipes[0].revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        check_cancelled()?;
        status(unsafe {
            renameat(
                self.dir_fd(),
                self.temp.as_ptr(),
                self.dir_fd(),
                self.dest.as_ptr(),
            )
        })?;
        Ok(())
    }

    fn sync_dir(&mut self) -> io::Result<()> {
        self.dir.as_ref().expect("directory open").sync_all()
    }

    fn unlink_temp(&mut self) -> io::Result<()> {
        let result = || status(unsafe { unlinkat(self.dir_fd(), self.temp.as_ptr(), 0) });
        loop {
            match result() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
                Ok(_) => return Ok(()),
            }
        }
    }
}

fn parse_digest(digest: &str) -> Option<[u8; 32]> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0; 32];
    for (byte, offset) in bytes.iter_mut().zip((0..64).step_by(2)) {
        *byte = u8::from_str_radix(&digest[offset..offset + 2], 16).ok()?;
    }
    Some(bytes)
}

fn arguments() -> Result<(Guest, u64, [u8; 32], u32), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [dest, length, digest, mode] = args.as_slice() else {
        return Err("usage: bput DEST LENGTH SHA256 MODE".into());
    };
    let length = length
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or("invalid length")?;
    let digest = digest
        .to_str()
        .and_then(parse_digest)
        .ok_or("invalid SHA-256")?;
    let mode = mode
        .to_str()
        .and_then(|s| u32::from_str_radix(s, 8).ok())
        .filter(|mode| *mode <= 0o777)
        .ok_or("mode must be octal 000 through 777")?;
    let path = Path::new(dest);
    let raw_name = path.as_os_str().as_bytes().rsplit(|&b| b == b'/').next();
    let name = raw_name
        .filter(|name| !matches!(*name, b"" | b"." | b".."))
        .ok_or("DEST needs a file name")?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let action = signal::Sigaction::new(cancel);
    for sig in [signal::SIGHUP, signal::SIGPIPE] {
        signal::sigaction(sig, Some(&action), None).map_err(|error| error.to_string())?;
    }
    let mask = (1 << (signal::SIGHUP - 1)) | (1 << (signal::SIGPIPE - 1));
    signal::sigprocmask(signal::SIG_UNBLOCK, Some(&mask), None)
        .map_err(|error| error.to_string())?;
    let mut nonce = [0; 16];
    let mut filled = 0;
    while filled < nonce.len() {
        match libbreenix::crypto::rand::getrandom_bytes(&mut nonce[filled..]) {
            Ok(n) if n > 0 => filled += n,
            _ => return Err("cannot get a temporary-file nonce".into()),
        }
    }
    let mut temp = Vec::from(b".".as_slice());
    temp.extend_from_slice(name);
    temp.extend_from_slice(b".bput.");
    for byte in nonce {
        write!(temp, "{byte:02x}").map_err(|error| error.to_string())?;
    }
    let guest = Guest {
        parent: parent.to_owned(),
        dest: CString::new(name).map_err(|error| error.to_string())?,
        temp: CString::new(temp).map_err(|error| error.to_string())?,
        dir: None,
    };
    Ok((guest, length, digest, mode))
}

pub(super) fn main() -> ExitCode {
    let result = arguments()
        .map_err(|reason| Failure { status: 1, reason })
        .and_then(|(mut guest, length, digest, mode)| publish(&mut guest, length, digest, mode));
    match result {
        Ok(()) => {
            let _ = writeln!(io::stdout().lock(), "published");
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let outcome = if failure.status == 3 {
                "published"
            } else {
                "not published"
            };
            let _ = writeln!(io::stderr().lock(), "bput: {outcome}; {}", failure.reason);
            ExitCode::from(failure.status)
        }
    }
}
