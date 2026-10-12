//! Serialize shared disk boots and recover a runner group after an uncatchable exit.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use super::process_tree::ChildGroup;
use super::signal_handlers::SignalHandlers;

pub struct BootLock {
    file: File,
}

fn birth(pid: libc::pid_t) -> io::Result<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

impl BootLock {
    pub fn acquire(path: &str) -> io::Result<Self> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let deadline = Instant::now() + Duration::from_secs(600);
        loop {
            if SignalHandlers::interrupted() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "QEMU test interrupted waiting for lock",
                ));
            }
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(error);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "shared QEMU boot lock deadline",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
        let mut record = String::new();
        file.read_to_string(&mut record)?;
        if let Some((pid, started)) = record.trim().split_once('\t') {
            let pid: libc::pid_t = pid
                .parse()
                .map_err(|_| io::Error::other("invalid stale QEMU group"))?;
            if pid <= 1 || started.is_empty() {
                return Err(io::Error::other("invalid stale QEMU identity"));
            }
            let current = birth(pid)?;
            // A surviving leader must still be the recorded process. With no
            // leader, the group's surviving descendants reserve its group ID.
            if current.is_empty() || current == started {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
                let deadline = Instant::now() + Duration::from_secs(10);
                while unsafe { libc::kill(-pid, 0) } == 0 {
                    if Instant::now() >= deadline {
                        return Err(io::Error::other("stale QEMU group did not exit"));
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
        file.set_len(0)?;
        file.rewind()?;
        Ok(Self { file })
    }

    pub fn spawn(&mut self, command: &mut Command) -> io::Result<ChildGroup> {
        let group = ChildGroup::spawn(command)?;
        let started = birth(group.id())?;
        if started.is_empty() {
            return Err(io::Error::other("cannot identify QEMU group leader"));
        }
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        writeln!(self.file, "{}\t{}", group.id(), started)?;
        self.file.sync_data()?;
        Ok(group)
    }
}

impl Drop for BootLock {
    fn drop(&mut self) {
        // ChildGroup is declared after this guard and is reaped first.
        let _ = self.file.set_len(0);
    }
}
