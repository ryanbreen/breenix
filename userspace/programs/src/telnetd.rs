//! Telnet server - spawns a shell per connection via PTY (std version)
//!
//! - Binds to port 2323
//! - Accepts TCP connections
//! - Creates a PTY pair for each connection
//! - Forks a child with stdin/stdout/stderr on the PTY slave
//! - Parent relays data between socket and PTY master
//! - Provides proper terminal semantics (line discipline, ioctls)

use libbreenix::io::{close, dup2, fcntl_getfl, fcntl_setfl, read, write, status_flags::O_NONBLOCK};
use libbreenix::process::{fork, waitpid, yield_now, ForkResult, WNOHANG};
use libbreenix::pty;
use libbreenix::socket::{socket, bind_inet, listen, accept, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::types::Fd;

const TELNET_PORT: u16 = 2323;
const SHELL_PATH: &[u8] = b"/bin/bsh\0";

// setsockopt constants (not yet in libbreenix)
const SOL_SOCKET: i32 = 1;
const SO_REUSEADDR: i32 = 2;
use libbreenix::syscall::nr;
const SYS_SETSOCKOPT: u64 = nr::SETSOCKOPT;

/// Set a file descriptor to non-blocking mode
fn set_nonblocking(fd: Fd) {
    if let Ok(flags) = fcntl_getfl(fd) {
        let _ = fcntl_setfl(fd, flags as i32 | O_NONBLOCK);
    }
}

/// Raw setsockopt (not yet in libbreenix)
fn setsockopt(fd: Fd, level: i32, optname: i32, optval: &i32) {
    unsafe {
        libbreenix::raw::syscall5(
            SYS_SETSOCKOPT,
            fd.raw(),
            level as u64,
            optname as u64,
            optval as *const i32 as u64,
            core::mem::size_of::<i32>() as u64,
        );
    }
}

// Telnet protocol constants
const IAC: u8 = 0xFF;
const WILL: u8 = 0xFB;
const WONT: u8 = 0xFC;
const DO: u8 = 0xFD;
const DONT: u8 = 0xFE;
const SB: u8 = 0xFA;
const SE: u8 = 0xF0;

/// Telnet IAC parser state
#[derive(Clone, Copy, PartialEq)]
enum TelnetState {
    /// Normal data mode
    Data,
    /// Saw IAC byte, waiting for command
    Iac,
    /// Saw IAC WILL/WONT/DO/DONT, waiting for option byte
    Option(u8),
    /// Inside sub-negotiation (IAC SB ... IAC SE)
    SubNeg,
    /// Saw IAC inside sub-negotiation (might be SE)
    SubNegIac,
}

/// Filter Telnet IAC sequences from client data.
///
/// Strips all IAC command/option bytes so only user data reaches the PTY.
/// Sends WONT/DONT responses to refuse all option negotiations.
/// Returns the number of clean data bytes written to `out`.
fn filter_telnet(
    data: &[u8],
    out: &mut [u8],
    state: &mut TelnetState,
    client_fd: Fd,
) -> usize {
    let mut out_pos = 0;
    for &byte in data {
        match *state {
            TelnetState::Data => {
                if byte == IAC {
                    *state = TelnetState::Iac;
                } else {
                    if out_pos < out.len() {
                        out[out_pos] = byte;
                        out_pos += 1;
                    }
                }
            }
            TelnetState::Iac => {
                match byte {
                    WILL | WONT => {
                        // Next byte is the option code; we'll send DONT
                        *state = TelnetState::Option(byte);
                    }
                    DO | DONT => {
                        // Next byte is the option code; we'll send WONT
                        *state = TelnetState::Option(byte);
                    }
                    SB => {
                        *state = TelnetState::SubNeg;
                    }
                    IAC => {
                        // IAC IAC = literal 0xFF byte
                        if out_pos < out.len() {
                            out[out_pos] = 0xFF;
                            out_pos += 1;
                        }
                        *state = TelnetState::Data;
                    }
                    _ => {
                        // Other 2-byte IAC commands (NOP, BRK, etc.) - consume
                        *state = TelnetState::Data;
                    }
                }
            }
            TelnetState::Option(cmd) => {
                // byte is the option code - refuse the negotiation
                let response_cmd = if cmd == WILL || cmd == WONT { DONT } else { WONT };
                let resp = [IAC, response_cmd, byte];
                let _ = write(client_fd, &resp);
                *state = TelnetState::Data;
            }
            TelnetState::SubNeg => {
                if byte == IAC {
                    *state = TelnetState::SubNegIac;
                }
                // else: consume sub-negotiation data
            }
            TelnetState::SubNegIac => {
                if byte == SE {
                    *state = TelnetState::Data;
                } else {
                    // Not SE, back to sub-negotiation data
                    *state = TelnetState::SubNeg;
                }
            }
        }
    }
    out_pos
}

/// Relay data between socket and PTY master using non-blocking I/O.
/// Returns when the child process exits.
fn relay_loop(client_fd: Fd, master_fd: Fd, child_pid: i32) {
    set_nonblocking(client_fd);
    set_nonblocking(master_fd);

    let mut buf = [0u8; 1024];
    let mut filtered = [0u8; 1024];
    let mut telnet_state = TelnetState::Data;

    loop {
        // Check if child has exited
        let mut status: i32 = 0;
        if let Ok(pid) = waitpid(child_pid, &mut status as *mut i32, WNOHANG) {
            if pid.raw() > 0 {
                // Child exited - drain any remaining PTY master data to socket
                loop {
                    match read(master_fd, &mut buf) {
                        Ok(n) if n > 0 => { let _ = write(client_fd, &buf[..n]); }
                        _ => break,
                    }
                }
                return;
            }
        }

        let mut did_work = false;

        // Socket -> PTY master (client input to shell)
        // Filter out Telnet IAC protocol bytes so only user data reaches the PTY
        match read(client_fd, &mut buf) {
            Ok(n) if n > 0 => {
                let clean = filter_telnet(
                    &buf[..n],
                    &mut filtered,
                    &mut telnet_state,
                    client_fd,
                );
                if clean > 0 {
                    let _ = write(master_fd, &filtered[..clean]);
                }
                did_work = true;
            }
            Ok(0) => {
                // Client disconnected
                return;
            }
            _ => {}
        }

        // PTY master -> socket (shell output to client)
        match read(master_fd, &mut buf) {
            Ok(n) if n > 0 => {
                let _ = write(client_fd, &buf[..n]);
                did_work = true;
            }
            _ => {}
        }

        if !did_work {
            let _ = yield_now();
        }
    }
}

/// Handle a single telnet connection using a PTY pair
fn handle_connection(client_fd: Fd) {
    // Create PTY pair using libbreenix convenience function
    let (master_fd, path_buf) = match pty::openpty() {
        Ok(pair) => pair,
        Err(_) => {
            print!("TELNETD_ERROR: openpty failed\n");
            let _ = close(client_fd);
            return;
        }
    };

    // Fork child process for shell
    match fork() {
        Ok(ForkResult::Child) => {
            // Child process: open slave PTY and redirect stdin/stdout/stderr

            // Close fds the child doesn't need
            let _ = close(master_fd);
            let _ = close(client_fd);

            // Open the slave PTY device
            let slave_path = core::str::from_utf8(pty::slave_path_bytes(&path_buf)).unwrap_or("");
            // Need null-terminated path for open
            let mut path_with_null = String::from(slave_path);
            path_with_null.push('\0');
            let slave_fd = match libbreenix::fs::open(&path_with_null, libbreenix::fs::O_RDWR) {
                Ok(fd) => fd,
                Err(_) => {
                    let msg = b"TELNETD_CHILD: open slave failed\n";
                    let _ = write(Fd::STDOUT, msg);
                    std::process::exit(10);
                }
            };

            // Redirect stdin/stdout/stderr to the PTY slave
            if dup2(slave_fd, Fd::from_raw(0)).is_err() {
                let msg = b"TELNETD_CHILD: dup2 stdin failed\n";
                let _ = write(Fd::STDOUT, msg);
                std::process::exit(11);
            }
            if dup2(slave_fd, Fd::from_raw(1)).is_err() {
                let msg = b"TELNETD_CHILD: dup2 stdout failed\n";
                let _ = write(Fd::STDERR, msg);
                std::process::exit(12);
            }
            if dup2(slave_fd, Fd::from_raw(2)).is_err() {
                std::process::exit(13);
            }

            // Close original slave_fd (now duplicated to 0/1/2)
            if slave_fd.raw() > 2 {
                let _ = close(slave_fd);
            }

            // Execute shell
            let argv: [*const u8; 2] = [SHELL_PATH.as_ptr(), core::ptr::null()];
            // envp was just [null] (empty environment), so execv (no envp arg) is equivalent
            let _ = libbreenix::process::execv(SHELL_PATH, argv.as_ptr());

            // If exec fails
            std::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let pid = child_pid.raw() as i32;

            print!("TELNETD_SHELL_FORKED\n");

            // Parent: relay data between socket and PTY master
            relay_loop(client_fd, master_fd, pid);

            // Clean up
            let _ = close(master_fd);
            let _ = close(client_fd);

            // Ensure child is reaped
            let mut status: i32 = 0;
            let _ = waitpid(pid, &mut status as *mut i32, 0);

            print!("TELNETD_SESSION_ENDED\n");
        }
        Err(_) => {
            print!("TELNETD_ERROR: fork failed\n");
            let _ = close(master_fd);
            let _ = close(client_fd);
        }
    }
}

fn main() {
    print!("TELNETD_STARTING\n");

    // Create listening socket
    let listen_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            print!("TELNETD_ERROR: socket failed\n");
            std::process::exit(1);
        }
    };

    // Set SO_REUSEADDR
    let optval: i32 = 1;
    setsockopt(listen_fd, SOL_SOCKET, SO_REUSEADDR, &optval);

    // Bind to port
    let addr = SockAddrIn::new([0, 0, 0, 0], TELNET_PORT);
    if bind_inet(listen_fd, &addr).is_err() {
        print!("TELNETD_ERROR: bind failed\n");
        std::process::exit(1);
    }

    // Start listening
    if listen(listen_fd, 128).is_err() {
        print!("TELNETD_ERROR: listen failed\n");
        std::process::exit(1);
    }

    print!("TELNETD_LISTENING\n");

    // Accept connections forever (daemon mode)
    loop {
        match accept(listen_fd, None) {
            Ok(client_fd) => {
                print!("TELNETD_CONNECTED\n");
                handle_connection(client_fd);
                print!("TELNETD_LISTENING\n");
            }
            Err(_) => {
                // EAGAIN or other error - just yield and retry
                let _ = yield_now();
            }
        }
    }
}
