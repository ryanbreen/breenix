//! Telnet server using PTY infrastructure
//!
//! - Binds to port 2323
//! - Accepts a single TCP connection
//! - Creates a PTY pair and forks a shell attached to the slave
//! - Relays data between socket and PTY master using poll()

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::fs;
use libbreenix::io::{self, close, dup2, poll, PollFd};
use libbreenix::io::poll_events::{POLLERR, POLLHUP, POLLIN, POLLNVAL};
use libbreenix::process;
use libbreenix::pty;
use libbreenix::signal;
use libbreenix::socket::{accept, bind, listen, socket, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::types::fd;

const TELNET_PORT: u16 = 2323;
const EAGAIN: i64 = -11;
const SHELL_PATH: &[u8] = b"/bin/init_shell\0";

/// Write all data to fd, handling partial writes
fn write_all(file: u64, buf: &[u8]) -> i64 {
    let mut offset = 0usize;
    while offset < buf.len() {
        let n = io::write(file, &buf[offset..]);
        if n <= 0 {
            return n;
        }
        offset += n as usize;
    }
    offset as i64
}

/// Open the PTY slave device given the path buffer from openpty()
fn open_slave(slave_path: &[u8; 32]) -> Option<u64> {
    // Find the null terminator to get path length
    let path_bytes = pty::slave_path_bytes(slave_path);

    // Create a null-terminated string for open()
    // Path format is "/dev/pts/N" where N is 0-255, so max 12 chars + null
    let mut path_buf = [0u8; 16];
    let path_len = path_bytes.len().min(path_buf.len() - 1);
    path_buf[..path_len].copy_from_slice(&path_bytes[..path_len]);
    path_buf[path_len] = 0;

    // Convert to &str for open()
    let path_str = match core::str::from_utf8(&path_buf[..path_len + 1]) {
        Ok(s) => s,
        Err(_) => return None,
    };

    match fs::open(path_str, fs::O_RDWR) {
        Ok(fd) => Some(fd),
        Err(_) => None,
    }
}

/// Terminate the child process gracefully
fn terminate_child(pid: i32) {
    // Try SIGTERM first
    let _ = signal::kill(pid, signal::SIGTERM);

    // Wait a bit for graceful shutdown
    for _ in 0..100 {
        let status = process::waitpid(pid, core::ptr::null_mut(), process::WNOHANG);
        if status == pid as i64 {
            return;
        }
        process::yield_now();
    }

    // Force kill if still alive
    let _ = signal::kill(pid, signal::SIGKILL);

    // Wait for termination
    for _ in 0..100 {
        let status = process::waitpid(pid, core::ptr::null_mut(), process::WNOHANG);
        if status == pid as i64 {
            return;
        }
        process::yield_now();
    }
}

/// Handle a single telnet connection
fn handle_connection(client_fd: i32) {
    // Create PTY pair
    let (master_fd, slave_path) = match pty::openpty() {
        Ok(pair) => pair,
        Err(_) => {
            io::print("TELNETD_ERROR: openpty failed\n");
            close(client_fd as u64);
            return;
        }
    };

    io::print("TELNETD_PTY_CREATED\n");

    // Fork child process for shell
    let pid = process::fork();

    if pid == 0 {
        // Child process: set up PTY slave as stdin/stdout/stderr and exec shell
        close(master_fd as u64);
        close(client_fd as u64);

        // Open the PTY slave
        let slave_fd = match open_slave(&slave_path) {
            Some(fd) => fd,
            None => {
                io::print("TELNETD_ERROR: open slave failed\n");
                process::exit(1);
            }
        };

        // Redirect stdin/stdout/stderr to PTY slave
        if dup2(slave_fd, fd::STDIN) < 0 {
            process::exit(1);
        }
        if dup2(slave_fd, fd::STDOUT) < 0 {
            process::exit(1);
        }
        if dup2(slave_fd, fd::STDERR) < 0 {
            process::exit(1);
        }
        close(slave_fd);

        // Execute shell
        let argv: [*const u8; 2] = [SHELL_PATH.as_ptr(), core::ptr::null()];
        let _ = process::execv(SHELL_PATH, argv.as_ptr());

        // If exec fails
        process::exit(127);
    }

    if pid < 0 {
        io::print("TELNETD_ERROR: fork failed\n");
        close(master_fd as u64);
        close(client_fd as u64);
        return;
    }

    io::print("TELNETD_SHELL_FORKED\n");

    // Parent: relay data between socket and PTY master
    let mut fds = [
        PollFd::new(client_fd, POLLIN),
        PollFd::new(master_fd, POLLIN),
    ];

    let mut buf = [0u8; 1024];
    loop {
        // Clear revents before polling
        for pfd in &mut fds {
            pfd.revents = 0;
        }

        let ready = poll(&mut fds, 1000); // 1 second timeout

        if ready < 0 {
            break;
        }

        // Check for errors on client socket
        if fds[0].revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            break;
        }

        // Check for errors on PTY master
        if fds[1].revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            break;
        }

        // Read from client, write to PTY master
        if fds[0].revents & POLLIN != 0 {
            let n = io::read(client_fd as u64, &mut buf);
            if n > 0 {
                if write_all(master_fd as u64, &buf[..n as usize]) < 0 {
                    break;
                }
            } else if n == 0 {
                break; // Client disconnected
            } else if n != EAGAIN {
                break;
            }
        }

        // Read from PTY master, write to client
        if fds[1].revents & POLLIN != 0 {
            let n = io::read(master_fd as u64, &mut buf);
            if n > 0 {
                if write_all(client_fd as u64, &buf[..n as usize]) < 0 {
                    break;
                }
            } else if n == 0 {
                break; // PTY closed
            } else if n != EAGAIN {
                break;
            }
        }
    }

    io::print("TELNETD_RELAY_DONE\n");

    close(master_fd as u64);
    close(client_fd as u64);
    terminate_child(pid as i32);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("TELNETD_STARTING\n");

    // Create listening socket
    let listen_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TELNETD_ERROR: socket failed\n");
            process::exit(1);
        }
    };

    // Bind to port
    let addr = SockAddrIn::new([0, 0, 0, 0], TELNET_PORT);
    if bind(listen_fd, &addr).is_err() {
        io::print("TELNETD_ERROR: bind failed\n");
        process::exit(1);
    }

    // Start listening
    if listen(listen_fd, 128).is_err() {
        io::print("TELNETD_ERROR: listen failed\n");
        process::exit(1);
    }

    io::print("TELNETD_LISTENING\n");

    // Accept connections forever (daemon mode)
    loop {
        match accept(listen_fd, None) {
            Ok(client_fd) => {
                io::print("TELNETD_CONNECTED\n");
                handle_connection(client_fd);
                // After connection closes, continue accepting
                io::print("TELNETD_LISTENING\n");
            }
            Err(_) => {
                // EAGAIN or other error - just yield and retry
                process::yield_now();
            }
        }
    }

    // Unreachable in daemon mode, but keep for completeness
    #[allow(unreachable_code)]
    {
        io::print("TELNETD_SHUTDOWN\n");
        process::exit(0);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("TELNETD_PANIC\n");
    process::exit(101);
}
