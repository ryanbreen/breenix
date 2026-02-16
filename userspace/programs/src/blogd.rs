//! blogd — Breenix Log Daemon
//!
//! Background daemon started by init. Reads kernel log messages from
//! /proc/kmsg (a non-destructive 32KB ring buffer) and appends new
//! data to /var/log/kernel.log on the ext2 filesystem.
//!
//! Loop:
//!   1. Open /proc/kmsg, read full buffer
//!   2. Compare with last known offset to find new data
//!   3. Append new bytes to /var/log/kernel.log
//!   4. Sleep 1 second
//!
//! Handles ring buffer wrap (new total < previous offset → reset).

use libbreenix::fs;
use libbreenix::io;
use libbreenix::time;
use libbreenix::types::Timespec;

const KMSG_PATH: &str = "/proc/kmsg";
const LOG_PATH: &str = "/var/log/kernel.log";
const VAR_DIR: &str = "/var";
const VAR_LOG_DIR: &str = "/var/log";

/// Read the full contents of /proc/kmsg into a Vec.
fn read_kmsg() -> Vec<u8> {
    let mut buf = vec![0u8; 4096];
    match fs::open(KMSG_PATH, fs::O_RDONLY) {
        Ok(fd) => {
            let mut total = 0;
            loop {
                match io::read(fd, &mut buf[total..]) {
                    Ok(n) if n > 0 => {
                        total += n;
                        if total >= buf.len() - 256 {
                            buf.resize(buf.len() + 4096, 0);
                        }
                    }
                    _ => break,
                }
            }
            let _ = io::close(fd);
            buf.truncate(total);
            buf
        }
        Err(_) => Vec::new(),
    }
}

/// Ensure /var/log/ directory exists. Returns true if the directory is ready.
fn ensure_log_dir() -> bool {
    // Try to create /var first (may already exist, EEXIST is fine)
    let _ = fs::mkdir(VAR_DIR, 0o755);
    // Then /var/log
    let _ = fs::mkdir(VAR_LOG_DIR, 0o755);
    // Verify by trying to access it
    fs::access(VAR_LOG_DIR, fs::F_OK).is_ok()
}

/// Append data to the log file.
fn append_to_log(data: &[u8]) {
    match fs::open_with_mode(LOG_PATH, fs::O_WRONLY | fs::O_CREAT | fs::O_APPEND, 0o644) {
        Ok(fd) => {
            let _ = fs::write(fd, data);
            let _ = io::close(fd);
        }
        Err(_) => {
            // Can't write to log file — not much we can do
        }
    }
}

fn main() {
    print!("[blogd] Breenix log daemon starting\n");

    // Retry log directory creation — ext2 mount may not be ready yet
    let mut dir_ready = false;
    for attempt in 0..30 {
        if ensure_log_dir() {
            dir_ready = true;
            break;
        }
        if attempt == 0 {
            print!("[blogd] /var/log not ready, retrying...\n");
        }
        let _ = time::nanosleep(&Timespec { tv_sec: 1, tv_nsec: 0 });
    }

    if !dir_ready {
        print!("[blogd] ERROR: could not create /var/log after 30 attempts\n");
        // Continue anyway — append_to_log will fail gracefully
    } else {
        print!("[blogd] /var/log ready, logging to {}\n", LOG_PATH);
    }

    let mut last_offset: usize = 0;
    let sleep_duration = Timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };

    loop {
        let kmsg = read_kmsg();
        let total = kmsg.len();

        if total > 0 {
            if total < last_offset {
                // Ring buffer wrapped — write everything
                append_to_log(&kmsg);
                last_offset = total;
            } else if total > last_offset {
                // New data available
                append_to_log(&kmsg[last_offset..]);
                last_offset = total;
            }
            // If total == last_offset, no new data
        }

        let _ = time::nanosleep(&sleep_duration);
    }
}
