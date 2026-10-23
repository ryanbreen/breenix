//! BSSHD — Breenix SSH server daemon
//!
//! Listens for SSH connections on port 22 and runs each authenticated
//! session, a shell (with a PTY when one is requested) or an exec request, as
//! the account that logged in.
//!
//! - The host key is `/etc/ssh/ssh_host_ed25519_key`, generated from the
//!   kernel's random pool on the first start that finds no key; its SHA-256
//!   fingerprint is printed on each start. On a persistent install a new key
//!   is also kept in the machine state store, from which init restores it into
//!   each later generation's `/etc/ssh` (`libbreenix::ssh::machine_state`).
//! - Logins are publickey only, against the account's `~/.ssh/authorized_keys`
//!   (`libbreenix::ssh::account`). There is no password login, unknown users
//!   are refused, and root does not log in. The account must pass the accounts
//!   library's session checks (`libbreenix::accounts::session::validate`).
//! - Before a session runs, its process becomes the account through
//!   `session::become_account`, as login does: a PTY is given to the account
//!   (mode 0620, group tty) and returned to root when the session ends, then the
//!   groups, gid and uid, the umask from `/etc/login.defs`, the home directory,
//!   shell and environment. Each session start is recorded in `/var/log/auth.log`.
//!
//! Usage: bsshd [port]
//!   Default port: 22

use std::ffi::CString;
use std::path::Path;

use libbreenix::io;
use libbreenix::process;
use libbreenix::pty;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::accounts::{self, session};
use libbreenix::ssh::account::{self, Account};
use libbreenix::ssh::host_keys;
use libbreenix::ssh::machine_state;
use libbreenix::ssh::keys::{fingerprint_text, HostKey};
use libbreenix::ssh::transport::ServerSession;
use libbreenix::types::Fd;

const DEFAULT_PORT: u16 = 22;

extern "C" {
    fn setsockopt(fd: i32, level: i32, name: i32, value: *const u8, length: u32) -> i32;
}

/// Fill `buffer` from the kernel's random pool; getrandom without flags waits
/// until the pool is initialized.
fn credited_random(buffer: &mut [u8]) -> Result<(), &'static str> {
    let mut filled = 0;
    while filled < buffer.len() {
        match libbreenix::crypto::rand::getrandom_bytes(&mut buffer[filled..]) {
            Ok(0) | Err(_) => return Err("getrandom failed"),
            Ok(n) => filled += n,
        }
    }
    Ok(())
}

/// Load or generate the host key, keep a new one in the machine state store,
/// and print its fingerprint.
fn prepare() -> HostKey {
    let root = Path::new("/");
    let key = match host_keys::load_or_generate(root, credited_random) {
        Ok((key, generated)) => {
            println!(
                "bsshd: host key ED25519 {} ({})",
                fingerprint_text(&key.public_key_blob()),
                if generated { "generated" } else { "loaded" }
            );
            key
        }
        Err(why) => {
            eprintln!("bsshd: no host key: {}", why);
            std::process::exit(1);
        }
    };
    match machine_state::keep_host_key(root) {
        Ok(true) => println!("bsshd: host key kept in the machine state store"),
        Ok(false) => {}
        Err(why) => eprintln!("bsshd: host key not kept in the machine state store: {}", why),
    }
    key
}

/// In a session's child process: become the account, enter its home and
/// return its environment. The caller exits instead of running anything when
/// this fails.
fn enter_account(account: &Account, term: Option<&str>) -> Result<Vec<CString>, String> {
    session::become_account(account, &session::LoginDefs::read(Path::new("/")), None)?;
    let home = CString::new(account.home.as_str()).map_err(|_| "invalid home directory".to_string())?;
    if process::chdir(home.as_bytes_with_nul()).is_err() {
        eprintln!("bsshd: home directory {} is not available; starting in /", account.home);
        let _ = process::chdir(b"/\0");
    }
    session::login_environment(account, term)
        .into_iter()
        .map(|entry| CString::new(entry).map_err(|_| "invalid environment".to_string()))
        .collect()
}

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

    // A restarted bsshd binds while its predecessor's sessions, and connections in
    // TIME_WAIT, still use the port; SO_REUSEADDR permits that (a second listener is
    // still refused).
    const SOL_SOCKET: i32 = 1;
    const SO_REUSEADDR: i32 = 2;
    let on: i32 = 1;
    if unsafe { setsockopt(listen_fd.raw() as i32, SOL_SOCKET, SO_REUSEADDR, (&on as *const i32).cast(), 4) } != 0 {
        eprintln!("bsshd: setsockopt(SO_REUSEADDR) failed: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
    let addr = SockAddrIn::new([0, 0, 0, 0], port);
    if let Err(e) = socket::bind_inet(listen_fd, &addr) {
        eprintln!("bsshd: bind() failed: {:?}", e);
        std::process::exit(1);
    }

    if let Err(e) = socket::listen(listen_fd, 5) {
        eprintln!("bsshd: listen() failed: {:?}", e);
        std::process::exit(1);
    }

    // Listen before the host key is read or generated, which can wait for the
    // random pool: early clients queue in the backlog rather than being refused.
    let host_key = prepare();
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
                handle_connection(client_fd, &host_key);
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
fn handle_connection(fd: Fd, host_key: &HostKey) {
    let mut authorizer = account::FileAuthorizer::new("/");
    let mut session = ServerSession::new(fd, host_key, &mut authorizer);

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
    let account = match session::validate(Path::new("/"), &username)
        .map_err(account::Refusal)
        .and_then(|entry| account::may_log_in(&entry.account).map(|_| entry.account))
    {
        Ok(account) => account,
        Err(why) => {
            eprintln!("bsshd: session refused: {}", why);
            session.close();
            return;
        }
    };

    let record = session::Record { program: "bsshd", user: &account.name, target: &account.name, tty: "-", refused: None, cmd: "" };
    if let Err(why) = accounts::log(Path::new("/"), &record) {
        eprintln!("bsshd: auth log: {}", why);
    }

    // Wait for channel open + shell request
    let pty_requested = match session.wait_for_channel() {
        Ok(pty) => pty,
        Err(e) => {
            eprintln!("bsshd: channel setup failed: {:?}", e);
            return;
        }
    };

    if let Some(command) = session.take_exec_command() {
        let status = run_exec_command(&mut session, &account, &command);
        if let Err(e) = session.finish(status) {
            eprintln!("bsshd: exec close failed: {:?}", e);
        }
        return;
    }

    let term = session.pty_term();

    // Shell input and output are separate pipes without a requested PTY.
    let mut child_pipe_fds = None;
    let (master_fd, input_fd, slave_path) = if pty_requested {
        match pty::openpty() {
            Ok((master, path)) => (master, master, path),
            Err(e) => {
                eprintln!("bsshd: openpty() failed: {:?}", e);
                return;
            }
        }
    } else {
        let (input_read, input_write) = match io::pipe() {
            Ok(pipe) => pipe,
            Err(_) => return,
        };
        let (output_read, output_write) = match io::pipe() {
            Ok(pipe) => pipe,
            Err(_) => {
                let _ = io::close(input_read);
                let _ = io::close(input_write);
                return;
            }
        };
        child_pipe_fds = Some((input_read, output_write));
        (output_read, input_write, [0u8; 32])
    };

    // Fork a child for the shell
    match process::fork() {
        Ok(process::ForkResult::Child) => {
            // Child: close master, set up slave as stdin/stdout/stderr
            let _ = io::close(master_fd);
            if input_fd != master_fd {
                let _ = io::close(input_fd);
            }
            if let Some((input_read, output_write)) = child_pipe_fds {
                let _ = io::dup2(input_read, Fd::from_raw(0));
                let _ = io::dup2(output_write, Fd::from_raw(1));
                let _ = io::dup2(output_write, Fd::from_raw(2));
                let _ = io::close(input_read);
                let _ = io::close(output_write);
            }

            if pty_requested {
                // A PTY login owns a new session and its controlling terminal.
                if process::setsid().is_err() { process::exit(1); }
                // Open the slave PTY
                let path_bytes = pty::slave_path_bytes(&slave_path);
                let path_str = core::str::from_utf8(path_bytes).unwrap_or("/dev/pts/0");
                let slave_fd = match libbreenix::fs::open(path_str, libbreenix::fs::O_RDWR) {
                    Ok(fd) => fd,
                    Err(_) => process::exit(1),
                };

                if libbreenix::termios::set_controlling_terminal(slave_fd).is_err()
                    || libbreenix::termios::tcsetpgrp(slave_fd, process::getpid().unwrap().raw() as i32).is_err()
                    || session::grant_terminal(slave_fd.raw() as i32, &account).is_err()
                { process::exit(1); }

                // Set PTY to translate \n → \r\n (ONLCR) for SSH terminals
                let mut tio: libbreenix::termios::Termios = unsafe { core::mem::zeroed() };
                let _ = libbreenix::termios::tcgetattr(slave_fd, &mut tio);
                tio.c_oflag |=
                    libbreenix::termios::oflag::OPOST | libbreenix::termios::oflag::ONLCR;
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

            // Run the account's shell as the account, as a login shell.
            let environment = match enter_account(&account, term.as_deref()) {
                Ok(environment) => environment,
                Err(why) => {
                    eprintln!("bsshd: session for '{}' refused: {}", account.name, why);
                    process::exit(1);
                }
            };
            let mut envp: Vec<*const u8> = environment.iter().map(|entry| entry.as_ptr().cast()).collect();
            envp.push(core::ptr::null());
            let shell = c_string(&account.shell);
            let arg0 = c_string(&format!("-{}", account.shell.rsplit('/').next().unwrap_or("sh")));
            let argv = [arg0.as_ptr(), core::ptr::null()];
            let _ = process::execve(&shell, argv.as_ptr(), envp.as_ptr());
            eprintln!("bsshd: cannot run {}", account.shell);
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(child_pid)) => {
            // Parent: shuttle data between SSH channel and PTY master
            println!(
                "bsshd: shell started (pid {}) for user '{}'",
                child_pid.raw(),
                username
            );
            if let Some((input_read, output_write)) = child_pipe_fds {
                let _ = io::close(input_read);
                let _ = io::close(output_write);
            }
            let input_closed = data_shuttle(&mut session, master_fd, input_fd, pty_requested);
            println!("bsshd: session ended for user '{}'", username);

            // Restore ownership while the master still keeps the slave alive.
            if pty_requested {
                let name = core::str::from_utf8(pty::slave_path_bytes(&slave_path)).unwrap();
                if let Ok(slave) = libbreenix::fs::open(name, libbreenix::fs::O_RDWR | pty::O_NOCTTY as u32) {
                    if session::return_terminal(slave.raw() as i32).is_err() { eprintln!("bsshd: cannot restore PTY ownership"); }
                    let _ = io::close(slave);
                }
            }
            // Clean up
            let _ = io::close(master_fd);
            if input_fd != master_fd && !input_closed {
                let _ = io::close(input_fd);
            }
            let mut status = 0;
            let shell_status = match process::waitpid(child_pid.raw() as i32, &mut status, 0) {
                Ok(_) if process::wifexited(status) => process::wexitstatus(status),
                Ok(_) if process::wifsignaled(status) => 128 + process::wtermsig(status),
                _ => 1,
            };
            if let Err(e) = session.finish(shell_status) {
                eprintln!("bsshd: shell close failed: {:?}", e);
            }
        }
        Err(e) => {
            eprintln!("bsshd: fork() for shell failed: {:?}", e);
            let _ = io::close(master_fd);
            if input_fd != master_fd {
                let _ = io::close(input_fd);
            }
            if let Some((input_read, output_write)) = child_pipe_fds {
                let _ = io::close(input_read);
                let _ = io::close(output_write);
            }
        }
    }

    session.close();
}

fn run_exec_command(session: &mut ServerSession, account: &Account, command: &str) -> i32 {
    println!("bsshd: exec request");

    let (read_fd, write_fd) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(e) => {
            let msg = format!("bsshd: pipe failed: {:?}\n", e);
            let _ = session.send_data(msg.as_bytes());
            return 1;
        }
    };

    let (input_read, input_write) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(_) => {
            let _ = io::close(read_fd);
            let _ = io::close(write_fd);
            return 1;
        }
    };
    // Backpressure on command input must not prevent draining its output.
    if io::fcntl_setfl(input_write, io::status_flags::O_NONBLOCK).is_err() {
        for fd in [read_fd, write_fd, input_read, input_write] { let _ = io::close(fd); }
        return 1;
    }

    match process::fork() {
        Ok(process::ForkResult::Child) => {
            let _ = io::close(read_fd);
            let _ = io::close(input_write);
            let _ = io::dup2(input_read, Fd::from_raw(0));
            if input_read.raw() > 2 { let _ = io::close(input_read); }
            let _ = io::dup2(write_fd, Fd::from_raw(1));
            let _ = io::dup2(write_fd, Fd::from_raw(2));
            if write_fd.raw() > 2 {
                let _ = io::close(write_fd);
            }

            let environment = match enter_account(account, None) {
                Ok(environment) => environment,
                Err(why) => {
                    eprintln!("bsshd: session for '{}' refused: {}", account.name, why);
                    process::exit(1);
                }
            };
            let mut envp: Vec<*const u8> = environment.iter().map(|entry| entry.as_ptr().cast()).collect();
            envp.push(core::ptr::null());
            let path_c = c_string(&account.shell);
            let shell_name = account.shell.rsplit('/').next().unwrap_or(&account.shell);
            let arg_storage = [c_string(shell_name), c_string("-c"), c_string(command)];
            let mut argv: Vec<*const u8> = arg_storage.iter().map(|arg| arg.as_ptr()).collect();
            argv.push(core::ptr::null());
            let _ = process::execve(&path_c, argv.as_ptr(), envp.as_ptr());
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(child_pid)) => {
            let _ = io::close(write_fd);
            let _ = io::close(input_read);
            let relay = exec_shuttle(session, read_fd, input_write);
            let _ = io::close(read_fd);

            let mut status = 0i32;
            let status = match process::waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0) {
                Ok(_) if process::wifexited(status) => process::wexitstatus(status),
                Ok(_) if process::wifsignaled(status) => 128 + process::wtermsig(status),
                Ok(_) => 1,
                Err(_) => 1,
            };
            match relay {
                Ok(()) => status,
                Err(why) => {
                    eprintln!("bsshd: exec relay failed: {}", why);
                    1
                }
            }
        }
        Err(e) => {
            let _ = io::close(input_read);
            let _ = io::close(input_write);
            let _ = io::close(read_fd);
            let _ = io::close(write_fd);
            let msg = format!("bsshd: fork failed: {:?}\n", e);
            let _ = session.send_data(msg.as_bytes());
            1
        }
    }
}

/// RFC 4254 section 5.3: input EOF closes only stdin; output keeps flowing.
fn exec_shuttle(session: &mut ServerSession, output: Fd, input: Fd) -> Result<(), String> {
    use io::poll_events::{POLLERR, POLLHUP, POLLIN, POLLOUT};
    let mut pending = Vec::new();
    let mut offset = 0;
    let mut input_open = true;
    let result = (|| {
        loop {
            let capacity = session.send_capacity().min(4096);
            let mut fds = [
                io::PollFd::new(if capacity > 0 { output } else { Fd::from_raw(u64::MAX) }, POLLIN),
                io::PollFd::new(session.io().fd(), POLLIN),
                io::PollFd::new(if input_open { input } else { Fd::from_raw(u64::MAX) },
                               if pending.is_empty() { 0 } else { POLLOUT }),
            ];
            match io::poll(&mut fds, 100) {
                Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINTR)) => continue,
                Err(e) => return Err(format!("poll: {:?}", e)),
                Ok(_) => {}
            }
            if capacity > 0 && fds[0].revents & (POLLIN | POLLHUP) != 0 {
                let mut buf = [0u8; 4096];
                match io::read(output, &mut buf[..capacity]) {
                    Ok(0) => return Ok(()),
                    Ok(n) => session.send_data(&buf[..n]).map_err(|e| format!("send output: {:?}", e))?,
                    Err(e) => return Err(format!("read command output: {:?}", e)),
                }
            }
            if fds[2].revents & POLLOUT != 0 {
                match io::write(input, &pending[offset..]) {
                    Ok(0) => return Err(String::from("zero-byte stdin write")),
                    Ok(n) => {
                        session.consume_input(n).map_err(|e| format!("return input credit: {:?}", e))?;
                        offset += n;
                        if offset == pending.len() { pending.clear(); offset = 0; }
                    }
                    Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {}
                    Err(_) => {
                        let _ = io::close(input);
                        input_open = false;
                        session.consume_input(pending.len() - offset).map_err(|e| format!("discard input: {:?}", e))?;
                        pending.clear();
                        offset = 0;
                    }
                }
            }
            if fds[2].revents & (POLLERR | POLLHUP) != 0 && input_open {
                let _ = io::close(input);
                input_open = false;
                session.consume_input(pending.len() - offset).map_err(|e| format!("closed input: {:?}", e))?;
                pending.clear();
                offset = 0;
            }
            if fds[1].revents & POLLIN != 0 {
                match session.try_recv_data_uncredited() {
                    Ok(Some(data)) if input_open => {
                        pending.drain(..offset);
                        offset = 0;
                        pending.extend_from_slice(&data);
                    }
                    Ok(Some(data)) => session.consume_input(data.len()).map_err(|e| format!("discard data: {:?}", e))?,
                    Ok(None) => {}
                    Err(e) => return Err(format!("receive channel: {:?}", e)),
                }
            }
            if session.input_eof() && pending.is_empty() && input_open {
                let _ = io::close(input);
                input_open = false;
            }
            if fds[1].revents & (POLLERR | POLLHUP) != 0 {
                return Err(String::from("SSH socket closed before command output EOF"));
            }
        }
    })();
    if input_open { let _ = io::close(input); }
    result
}

fn c_string(value: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = value
        .as_bytes()
        .iter()
        .map(|byte| if *byte == 0 { b'?' } else { *byte })
        .collect();
    bytes.push(0);
    bytes
}

/// Shuttle data between the SSH channel and the PTY master fd.
///
/// Uses poll() to multiplex between:
/// - Data from the PTY master (shell output) → send to SSH client
/// - Data from the SSH client → write to PTY master (shell input)
fn data_shuttle(session: &mut ServerSession, master_fd: Fd, input_fd: Fd, has_pty: bool) -> bool {
    let mut input_eof = false;
    let ssh_fd = session.io().fd();

    loop {
        // Keep processing window adjustments while peer output credit is exhausted.
        let capacity = session.send_capacity().min(4096);
        let mut fds = [
            io::PollFd::new(if capacity > 0 { master_fd } else { Fd::from_raw(u64::MAX) }, io::poll_events::POLLIN),
            io::PollFd::new(ssh_fd, io::poll_events::POLLIN),
        ];

        // 100ms timeout — keeps the loop responsive
        match io::poll(&mut fds, 100) {
            Ok(0) => continue, // Timeout, no data on either fd
            Err(_) => break,
            _ => {}
        }

        // Check PTY master for shell output
        if capacity > 0 && fds[0].revents & io::poll_events::POLLIN != 0 {
            let mut pty_buf = [0u8; 4096];
            match io::read(master_fd, &mut pty_buf[..capacity]) {
                Ok(n) if n > 0 => {
                    if session.send_data(&pty_buf[..n]).is_err() {
                        break;
                    }
                }
                Ok(_) => break,
                Err(_) => break,
            }
        }

        // Check SSH socket for client input
        if fds[1].revents & io::poll_events::POLLIN != 0 {
            match session.recv_data() {
                Ok(Some(data)) => {
                    let _ = io::write(input_fd, &data);
                }
                Ok(None) => {
                    if session.input_eof() && !input_eof {
                        input_eof = true;
                        if has_pty {
                            let _ = io::write(input_fd, &[4]);
                        } else {
                            let _ = io::close(input_fd);
                        }
                    }
                }
                Err(libbreenix::ssh::SshError::Disconnected) => break,
                Err(_) => break,
            }
        }

        // Check for hangup/error on either fd
        if (capacity > 0 && fds[0].revents & io::poll_events::POLLHUP != 0
            && fds[0].revents & io::poll_events::POLLIN == 0)
            || fds[1].revents & io::poll_events::POLLHUP != 0
        {
            break;
        }
    }
    input_eof && !has_pty
}
