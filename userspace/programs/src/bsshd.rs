//! BSSHD — Breenix SSH server daemon
//!
//! Listens for SSH connections on port 2222 and spawns interactive shell
//! sessions with PTY allocation. Supports password authentication.
//!
//! Usage: bsshd [port]
//!   Default port: 2222

use libbreenix::io;
use libbreenix::process;
use libbreenix::pty;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::ssh::transport::ServerSession;
use libbreenix::types::Fd;

const DEFAULT_PORT: u16 = 2222;

fn main() {
    let port = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    println!("bsshd: starting on port {}", port);

    // Create TCP listening socket
    let listen_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("bsshd: socket() failed: {:?}", e);
            std::process::exit(1);
        }
    };

    let addr = SockAddrIn::new([0, 0, 0, 0], port);
    if let Err(e) = socket::bind_inet(listen_fd, &addr) {
        eprintln!("bsshd: bind() failed: {:?}", e);
        std::process::exit(1);
    }

    if let Err(e) = socket::listen(listen_fd, 5) {
        eprintln!("bsshd: listen() failed: {:?}", e);
        std::process::exit(1);
    }

    println!("bsshd: listening on 0.0.0.0:{}", port);

    // Accept loop
    loop {
        let mut client_addr = SockAddrIn::default();
        let client_fd = match socket::accept(listen_fd, Some(&mut client_addr)) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("bsshd: accept() failed: {:?}", e);
                continue;
            }
        };

        println!(
            "bsshd: connection from {}.{}.{}.{}:{}",
            client_addr.addr[0],
            client_addr.addr[1],
            client_addr.addr[2],
            client_addr.addr[3],
            client_addr.port_host()
        );

        // Fork a child to handle the connection
        match process::fork() {
            Ok(process::ForkResult::Child) => {
                let _ = io::close(listen_fd);
                handle_connection(client_fd);
                process::exit(0);
            }
            Ok(process::ForkResult::Parent(_child_pid)) => {
                let _ = io::close(client_fd);
                let _ = process::waitpid(-1, core::ptr::null_mut(), 1); // WNOHANG
            }
            Err(e) => {
                eprintln!("bsshd: fork() failed: {:?}", e);
                let _ = io::close(client_fd);
            }
        }
    }
}

/// Handle a single SSH connection.
fn handle_connection(fd: Fd) {
    let mut session = ServerSession::new(fd);

    let username = match session.handshake() {
        Ok(user) => {
            println!("bsshd: authenticated user '{}'", user);
            user
        }
        Err(e) => {
            eprintln!("bsshd: handshake failed: {:?}", e);
            return;
        }
    };

    // Wait for channel open + shell request
    let pty_requested = match session.wait_for_channel() {
        Ok(pty) => pty,
        Err(e) => {
            eprintln!("bsshd: channel setup failed: {:?}", e);
            return;
        }
    };

    // Allocate PTY if requested
    let (master_fd, slave_path) = if pty_requested {
        match pty::openpty() {
            Ok((master, path)) => (master, path),
            Err(e) => {
                eprintln!("bsshd: openpty() failed: {:?}", e);
                return;
            }
        }
    } else {
        // No PTY — use a pipe instead
        match io::pipe() {
            Ok((_r, w)) => {
                // Use the write end as "master" for simplicity
                let path = [0u8; 32];
                (w, path)
            }
            Err(e) => {
                eprintln!("bsshd: pipe() failed: {:?}", e);
                return;
            }
        }
    };

    // Fork a child for the shell
    match process::fork() {
        Ok(process::ForkResult::Child) => {
            // Child: close master, set up slave as stdin/stdout/stderr
            let _ = io::close(master_fd);

            if pty_requested {
                // Open the slave PTY
                let path_bytes = pty::slave_path_bytes(&slave_path);
                let path_str = core::str::from_utf8(path_bytes).unwrap_or("/dev/pts/0");
                let slave_fd = match libbreenix::fs::open(
                    path_str,
                    libbreenix::fs::O_RDWR,
                ) {
                    Ok(fd) => fd,
                    Err(_) => process::exit(1),
                };

                // Set PTY to translate \n → \r\n (ONLCR) for SSH terminals
                let mut tio: libbreenix::termios::Termios = unsafe { core::mem::zeroed() };
                let _ = libbreenix::termios::tcgetattr(slave_fd, &mut tio);
                tio.c_oflag |= libbreenix::termios::oflag::OPOST
                    | libbreenix::termios::oflag::ONLCR;
                tio.c_iflag |= libbreenix::termios::iflag::ICRNL;
                let _ = libbreenix::termios::tcsetattr(slave_fd, 0, &tio);

                // Redirect stdin/stdout/stderr to the slave PTY
                let _ = io::dup2(slave_fd, Fd::from_raw(0)); // stdin
                let _ = io::dup2(slave_fd, Fd::from_raw(1)); // stdout
                let _ = io::dup2(slave_fd, Fd::from_raw(2)); // stderr

                if slave_fd.raw() > 2 {
                    let _ = io::close(slave_fd);
                }
            }

            // Exec the shell
            let shell_path = b"/bin/bsh\0";
            let _ = process::exec(shell_path);
            // If exec fails, try init_shell
            let fallback = b"/bin/init_shell\0";
            let _ = process::exec(fallback);
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(child_pid)) => {
            // Parent: shuttle data between SSH channel and PTY master
            println!("bsshd: shell started (pid {}) for user '{}'", child_pid.raw(), username);
            data_shuttle(&mut session, master_fd);
            println!("bsshd: session ended for user '{}'", username);

            // Clean up
            let _ = io::close(master_fd);
            let _ = process::waitpid(child_pid.raw() as i32, core::ptr::null_mut(), 0);
        }
        Err(e) => {
            eprintln!("bsshd: fork() for shell failed: {:?}", e);
            let _ = io::close(master_fd);
        }
    }

    session.close();
}

/// Shuttle data between the SSH channel and the PTY master fd.
///
/// Uses poll() to multiplex between:
/// - Data from the PTY master (shell output) → send to SSH client
/// - Data from the SSH client → write to PTY master (shell input)
fn data_shuttle(session: &mut ServerSession, master_fd: Fd) {
    let ssh_fd = session.io().fd();

    loop {
        // Poll both the PTY master and SSH socket for readable data
        let mut fds = [
            io::PollFd::new(master_fd, io::poll_events::POLLIN),
            io::PollFd::new(ssh_fd, io::poll_events::POLLIN),
        ];

        // 100ms timeout — keeps the loop responsive
        match io::poll(&mut fds, 100) {
            Ok(0) => continue, // Timeout, no data on either fd
            Err(_) => break,
            _ => {}
        }

        // Check PTY master for shell output
        if fds[0].revents & io::poll_events::POLLIN != 0 {
            let mut pty_buf = [0u8; 4096];
            match io::read(master_fd, &mut pty_buf) {
                Ok(n) if n > 0 => {
                    if session.send_data(&pty_buf[..n]).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        // Check SSH socket for client input
        if fds[1].revents & io::poll_events::POLLIN != 0 {
            match session.recv_data() {
                Ok(Some(data)) => {
                    let _ = io::write(master_fd, &data);
                }
                Ok(None) => {}
                Err(libbreenix::ssh::SshError::Disconnected) => break,
                Err(_) => break,
            }
        }

        // Check for hangup/error on either fd
        if fds[0].revents & io::poll_events::POLLHUP != 0
            || fds[1].revents & io::poll_events::POLLHUP != 0
        {
            break;
        }
    }
}
