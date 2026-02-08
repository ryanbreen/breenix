//! Telnet server - spawns a shell per connection via PTY (std version)
//!
//! - Binds to port 2323
//! - Accepts TCP connections
//! - Creates a PTY pair for each connection
//! - Forks a child with stdin/stdout/stderr on the PTY slave
//! - Parent relays data between socket and PTY master
//! - Provides proper terminal semantics (line discipline, ioctls)

const TELNET_PORT: u16 = 2323;
const SHELL_PATH: &[u8] = b"/bin/init_shell\0";

// Socket constants
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOL_SOCKET: i32 = 1;
const SO_REUSEADDR: i32 = 2;

// Open flags
const O_RDWR: i32 = 0x02;
const O_NOCTTY: i32 = 0x100;
const O_NONBLOCK: i32 = 2048;

// fcntl commands
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;

// waitpid options
const WNOHANG: i32 = 1;

extern "C" {
    fn fork() -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn socket(domain: i32, typ: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const u8, addrlen: u32) -> i32;
    fn listen(fd: i32, backlog: i32) -> i32;
    fn accept(fd: i32, addr: *mut u8, addrlen: *mut u32) -> i32;
    fn setsockopt(fd: i32, level: i32, optname: i32, optval: *const u8, optlen: u32) -> i32;
    fn sched_yield() -> i32;
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname_r(fd: i32, buf: *mut u8, buflen: usize) -> i32;
    fn open(path: *const u8, flags: i32, mode: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, arg: i64) -> i32;
}

/// Set a file descriptor to non-blocking mode
fn set_nonblocking(fd: i32) {
    let flags = unsafe { fcntl(fd, F_GETFL, 0) };
    if flags >= 0 {
        unsafe { fcntl(fd, F_SETFL, (flags | O_NONBLOCK) as i64); }
    }
}

/// sockaddr_in structure (matching kernel layout)
#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16, // network byte order (big-endian)
    sin_addr: u32,
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: u32::from_ne_bytes(addr),
            sin_zero: [0u8; 8],
        }
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
    client_fd: i32,
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
                unsafe { write(client_fd, resp.as_ptr(), 3); }
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
fn relay_loop(client_fd: i32, master_fd: i32, child_pid: i32) {
    set_nonblocking(client_fd);
    set_nonblocking(master_fd);

    let mut buf = [0u8; 1024];
    let mut filtered = [0u8; 1024];
    let mut telnet_state = TelnetState::Data;

    loop {
        // Check if child has exited
        let mut status: i32 = 0;
        let ret = unsafe { waitpid(child_pid, &mut status as *mut i32, WNOHANG) };
        if ret > 0 {
            // Child exited - drain any remaining PTY master data to socket
            loop {
                let n = unsafe { read(master_fd, buf.as_mut_ptr(), buf.len()) };
                if n <= 0 {
                    break;
                }
                unsafe { write(client_fd, buf.as_ptr(), n as usize); }
            }
            return;
        }

        let mut did_work = false;

        // Socket -> PTY master (client input to shell)
        // Filter out Telnet IAC protocol bytes so only user data reaches the PTY
        let n = unsafe { read(client_fd, buf.as_mut_ptr(), buf.len()) };
        if n > 0 {
            let clean = filter_telnet(
                &buf[..n as usize],
                &mut filtered,
                &mut telnet_state,
                client_fd,
            );
            if clean > 0 {
                unsafe { write(master_fd, filtered.as_ptr(), clean); }
            }
            did_work = true;
        } else if n == 0 {
            // Client disconnected
            return;
        }

        // PTY master -> socket (shell output to client)
        let n = unsafe { read(master_fd, buf.as_mut_ptr(), buf.len()) };
        if n > 0 {
            unsafe { write(client_fd, buf.as_ptr(), n as usize); }
            did_work = true;
        }

        if !did_work {
            unsafe { sched_yield(); }
        }
    }
}

/// Handle a single telnet connection using a PTY pair
fn handle_connection(client_fd: i32) {
    // Create PTY pair
    let master_fd = unsafe { posix_openpt(O_RDWR | O_NOCTTY) };
    if master_fd < 0 {
        print!("TELNETD_ERROR: posix_openpt failed\n");
        unsafe { close(client_fd); }
        return;
    }

    if unsafe { grantpt(master_fd) } != 0 {
        print!("TELNETD_ERROR: grantpt failed\n");
        unsafe { close(master_fd); close(client_fd); }
        return;
    }

    if unsafe { unlockpt(master_fd) } != 0 {
        print!("TELNETD_ERROR: unlockpt failed\n");
        unsafe { close(master_fd); close(client_fd); }
        return;
    }

    let mut path_buf = [0u8; 32];
    if unsafe { ptsname_r(master_fd, path_buf.as_mut_ptr(), path_buf.len()) } != 0 {
        print!("TELNETD_ERROR: ptsname_r failed\n");
        unsafe { close(master_fd); close(client_fd); }
        return;
    }

    // Fork child process for shell
    let pid = unsafe { fork() };

    if pid == 0 {
        // Child process: open slave PTY and redirect stdin/stdout/stderr

        // Close fds the child doesn't need
        unsafe { close(master_fd); }
        unsafe { close(client_fd); }

        // Open the slave PTY device
        let slave_fd = unsafe { open(path_buf.as_ptr(), O_RDWR, 0) };
        if slave_fd < 0 {
            let msg = b"TELNETD_CHILD: open slave failed\n";
            unsafe { write(1, msg.as_ptr(), msg.len()); }
            std::process::exit(10);
        }

        // Redirect stdin/stdout/stderr to the PTY slave
        if unsafe { dup2(slave_fd, 0) } < 0 {
            let msg = b"TELNETD_CHILD: dup2 stdin failed\n";
            unsafe { write(1, msg.as_ptr(), msg.len()); }
            std::process::exit(11);
        }
        if unsafe { dup2(slave_fd, 1) } < 0 {
            // fd 1 still works here (dup2 failed so it wasn't replaced)
            let msg = b"TELNETD_CHILD: dup2 stdout failed\n";
            unsafe { write(2, msg.as_ptr(), msg.len()); }
            std::process::exit(12);
        }
        if unsafe { dup2(slave_fd, 2) } < 0 {
            std::process::exit(13);
        }

        // Close original slave_fd (now duplicated to 0/1/2)
        if slave_fd > 2 {
            unsafe { close(slave_fd); }
        }

        // Execute shell
        let argv: [*const u8; 2] = [SHELL_PATH.as_ptr(), std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];
        unsafe { execve(SHELL_PATH.as_ptr(), argv.as_ptr(), envp.as_ptr()); }

        // If exec fails
        std::process::exit(127);
    }

    if pid < 0 {
        print!("TELNETD_ERROR: fork failed\n");
        unsafe { close(master_fd); close(client_fd); }
        return;
    }

    print!("TELNETD_SHELL_FORKED\n");

    // Parent: relay data between socket and PTY master
    relay_loop(client_fd, master_fd, pid);

    // Clean up
    unsafe { close(master_fd); }
    unsafe { close(client_fd); }

    // Ensure child is reaped
    let mut status: i32 = 0;
    unsafe { waitpid(pid, &mut status as *mut i32, 0); }

    print!("TELNETD_SESSION_ENDED\n");
}

fn main() {
    print!("TELNETD_STARTING\n");

    // Create listening socket
    let listen_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if listen_fd < 0 {
        print!("TELNETD_ERROR: socket failed\n");
        std::process::exit(1);
    }

    // Set SO_REUSEADDR
    let optval: i32 = 1;
    unsafe {
        setsockopt(
            listen_fd,
            SOL_SOCKET,
            SO_REUSEADDR,
            &optval as *const i32 as *const u8,
            core::mem::size_of::<i32>() as u32,
        );
    }

    // Bind to port
    let addr = SockAddrIn::new([0, 0, 0, 0], TELNET_PORT);
    let ret = unsafe {
        bind(
            listen_fd,
            &addr as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>() as u32,
        )
    };
    if ret < 0 {
        print!("TELNETD_ERROR: bind failed\n");
        std::process::exit(1);
    }

    // Start listening
    let ret = unsafe { listen(listen_fd, 128) };
    if ret < 0 {
        print!("TELNETD_ERROR: listen failed\n");
        std::process::exit(1);
    }

    print!("TELNETD_LISTENING\n");

    // Accept connections forever (daemon mode)
    loop {
        let client_fd = unsafe { accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if client_fd >= 0 {
            print!("TELNETD_CONNECTED\n");
            handle_connection(client_fd);
            print!("TELNETD_LISTENING\n");
        } else {
            // EAGAIN or other error - just yield and retry
            unsafe { sched_yield(); }
        }
    }
}
