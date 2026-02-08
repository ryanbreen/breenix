//! Telnet server - spawns a shell per connection (std version)
//!
//! - Binds to port 2323
//! - Accepts TCP connections
//! - Forks a child with stdin/stdout/stderr redirected to the socket
//! - Child execs init_shell which communicates directly over TCP

const TELNET_PORT: u16 = 2323;
const SHELL_PATH: &[u8] = b"/bin/init_shell\0";

// Socket constants
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOL_SOCKET: i32 = 1;
const SO_REUSEADDR: i32 = 2;

extern "C" {
    fn fork() -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn socket(domain: i32, typ: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const u8, addrlen: u32) -> i32;
    fn listen(fd: i32, backlog: i32) -> i32;
    fn accept(fd: i32, addr: *mut u8, addrlen: *mut u32) -> i32;
    fn setsockopt(fd: i32, level: i32, optname: i32, optval: *const u8, optlen: u32) -> i32;
    fn sched_yield() -> i32;
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

/// Handle a single telnet connection by forking a shell
fn handle_connection(client_fd: i32) {
    // Fork child process for shell
    let pid = unsafe { fork() };

    if pid == 0 {
        // Child process: redirect stdin/stdout/stderr to the TCP socket

        // Redirect stdin/stdout/stderr to the TCP socket
        if unsafe { dup2(client_fd, 0) } < 0 {
            std::process::exit(1);
        }
        if unsafe { dup2(client_fd, 1) } < 0 {
            std::process::exit(1);
        }
        if unsafe { dup2(client_fd, 2) } < 0 {
            std::process::exit(1);
        }

        // Close the original client_fd (now duplicated to 0/1/2)
        if client_fd > 2 {
            unsafe { close(client_fd); }
        }

        // Execute shell
        let argv: [*const u8; 2] = [SHELL_PATH.as_ptr(), std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];
        unsafe { execve(SHELL_PATH.as_ptr(), argv.as_ptr(), envp.as_ptr()); }

        // If exec fails
        print!("TELNETD_ERROR: exec failed\n");
        std::process::exit(127);
    }

    if pid < 0 {
        print!("TELNETD_ERROR: fork failed\n");
        unsafe { close(client_fd); }
        return;
    }

    print!("TELNETD_SHELL_FORKED\n");

    // Parent: close client fd (child owns it now) and wait for child
    unsafe { close(client_fd); }

    // Wait for child to finish (blocking)
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
