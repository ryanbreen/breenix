//! Telnet server - spawns a shell per connection
//!
//! - Binds to port 2323
//! - Accepts TCP connections
//! - Forks a child with stdin/stdout/stderr redirected to the socket
//! - Child execs init_shell which communicates directly over TCP

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{self, close, dup2};
use libbreenix::process;
use libbreenix::socket::{accept, bind, listen, socket, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::types::fd;

const TELNET_PORT: u16 = 2323;
const SHELL_PATH: &[u8] = b"/bin/init_shell\0";

/// Handle a single telnet connection by forking a shell
fn handle_connection(client_fd: i32) {
    // Fork child process for shell
    let pid = process::fork();

    if pid == 0 {
        // Child process: redirect stdin/stdout/stderr to the TCP socket
        let cfd = client_fd as u64;

        // Redirect stdin/stdout/stderr to the TCP socket
        if dup2(cfd, fd::STDIN) < 0 {
            process::exit(1);
        }
        if dup2(cfd, fd::STDOUT) < 0 {
            process::exit(1);
        }
        if dup2(cfd, fd::STDERR) < 0 {
            process::exit(1);
        }

        // Close the original client_fd (now duplicated to 0/1/2)
        if client_fd as u64 > fd::STDERR {
            close(cfd);
        }

        // Execute shell
        let argv: [*const u8; 2] = [SHELL_PATH.as_ptr(), core::ptr::null()];
        let _ = process::execv(SHELL_PATH, argv.as_ptr());

        // If exec fails
        io::print("TELNETD_ERROR: exec failed\n");
        process::exit(127);
    }

    if pid < 0 {
        io::print("TELNETD_ERROR: fork failed\n");
        close(client_fd as u64);
        return;
    }

    io::print("TELNETD_SHELL_FORKED\n");

    // Parent: close client fd (child owns it now) and return to accept more
    close(client_fd as u64);

    // Wait for child to finish (blocking)
    let mut status: i32 = 0;
    let _ = process::waitpid(pid as i32, &mut status as *mut i32, 0);

    io::print("TELNETD_SESSION_ENDED\n");
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
                io::print("TELNETD_LISTENING\n");
            }
            Err(_) => {
                // EAGAIN or other error - just yield and retry
                process::yield_now();
            }
        }
    }

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
