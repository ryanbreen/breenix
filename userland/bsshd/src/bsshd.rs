//! BSSHD — Breenix SSH server daemon
//!
//! Listens for SSH connections on port 22 and runs each authenticated
//! shell, exec request or SFTP subsystem as the account that logged in.
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
//! Usage: bsshd [port] | bsshd --allow-peer ADDR [--lease-done PATH]
//! The restricted listener uses port 22 and expires after 45 seconds.

use std::ffi::CString;
use std::net::Ipv4Addr;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use libbreenix::{io, signal, termios};
use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::process;
use libbreenix::pty;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::accounts::{self, session};
use libbreenix::ssh::account::{self, Account};
use libbreenix::ssh::channel::SessionCommand;
use libbreenix::ssh::host_keys;
use libbreenix::ssh::machine_state;
use libbreenix::ssh::keys::{fingerprint_text, HostKey};
use libbreenix::ssh::transport::ServerSession;
use libbreenix::ssh::SshError;
use libbreenix::types::Fd;

const DEFAULT_PORT: u16 = 22;

mod bsshd_peer;
mod bsshd_sftp;

extern "C" {
    fn setsockopt(fd: i32, level: i32, name: i32, value: *const u8, length: u32) -> i32;
    fn geteuid() -> u32;
}

struct Lease {
    peer: Ipv4Addr,
    deadline: Instant,
    done: Option<PathBuf>,
    refused: u64,
    active: u32,
}

impl Lease {
    fn stop_reason(&self) -> Option<&'static str> {
        if Instant::now() >= self.deadline {
            Some("timeout")
        } else if self.done.as_ref().is_some_and(|path| path.is_file()) {
            Some("done")
        } else {
            None
        }
    }

    fn reap(&mut self) {
        while process::waitpid(-1, core::ptr::null_mut(), 1).is_ok_and(|pid| pid.raw() > 0) {
            self.active -= 1;
        }
    }

    fn finish(&mut self, listener: Fd, mut reason: &str) {
        let _ = io::close(listener);
        // Give the authenticated completion command time to send its SSH exit status.
        while self.active > 0 && Instant::now() < self.deadline {
            self.reap();
            if self.active > 0 { std::thread::sleep(Duration::from_millis(20)); }
        }
        if self.active > 0 { reason = "timeout"; }
        println!("bsshd: lease-end reason={reason} refused={}", self.refused);
    }
}

fn options() -> Result<(u16, Option<Lease>), &'static str> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, addr, ..] if flag == "--allow-peer" => {
            let peer: Ipv4Addr = addr.parse().map_err(|_| "--allow-peer needs one IPv4 address")?;
            if peer.is_unspecified() || peer.is_broadcast() || peer.is_multicast() {
                return Err("--allow-peer needs a unicast address");
            }
            let done = match &args[2..] {
                [] => None,
                [flag, path] if flag == "--lease-done" && Path::new(path).is_absolute() => Some(PathBuf::from(path)),
                _ => return Err("expected --lease-done with an absolute path"),
            };
            Ok((DEFAULT_PORT, Some(Lease { peer, deadline: Instant::now(), done, refused: 0, active: 0 })))
        }
        [] => Ok((DEFAULT_PORT, None)),
        [port] if !port.starts_with('-') => Ok((port.parse().unwrap_or(DEFAULT_PORT), None)),
        _ => Err("expected [port] or --allow-peer ADDR [--lease-done PATH]"),
    }
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
    match std::env::args().nth(1).as_deref() {
        Some("--sftp-server") => process::exit(run_sftp_worker(false)),
        Some("--sftp-server-openssh") => process::exit(run_sftp_worker(true)),
        _ => {}
    }
    let (port, mut lease) = options().unwrap_or_else(|error| {
        eprintln!("bsshd: {error}");
        std::process::exit(2);
    });

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

    // A leased listener must be able to close even if entropy or accept blocks.
    let leased_key = lease.as_ref().map(|_| prepare());
    if let Some(lease) = &mut lease {
        lease.deadline = Instant::now() + Duration::from_secs(45);
        if let Err(error) = io::fcntl_setfl(listen_fd, io::status_flags::O_NONBLOCK) {
            eprintln!("bsshd: nonblocking listener: {error}");
            std::process::exit(1);
        }
    }
    if let Err(e) = socket::listen(listen_fd, 5) {
        eprintln!("bsshd: listen() failed: {:?}", e);
        std::process::exit(1);
    }

    // Listen before the host key is read or generated, which can wait for the
    // random pool: early clients queue in the backlog rather than being refused.
    let host_key = leased_key.unwrap_or_else(prepare);
    println!("bsshd: listening on 0.0.0.0:{}", port);

    // Accept loop
    loop {
        if let Some(lease) = &mut lease {
            lease.reap();
            if let Some(reason) = lease.stop_reason() {
                lease.finish(listen_fd, reason);
                return;
            }
        }
        let mut client_addr = SockAddrIn::default();
        let client_fd = match socket::accept(listen_fd, Some(&mut client_addr)) {
            Ok(fd) => fd,
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) if lease.is_some() => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(e) => {
                eprintln!("bsshd: accept() failed: {:?}", e);
                continue;
            }
        };

        let client_fd = if let Some(lease) = &mut lease {
            // SAFETY: accept returned a new descriptor that this process owns.
            let client = unsafe { OwnedFd::from_raw_fd(client_fd.raw() as i32) };
            let Some(client) = bsshd_peer::admit(client, client_addr.addr.into(), lease.peer, &mut lease.refused) else {
                continue;
            };
            Fd::from_raw(client.into_raw_fd() as u64)
        } else {
            client_fd
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
                if let Some(lease) = &mut lease {
                    lease.active += 1;
                } else {
                    let _ = process::waitpid(-1, core::ptr::null_mut(), 1); // WNOHANG
                }
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

    loop {
        let result = session.wait_for_channel()
            .and_then(|command| run_channel(&mut session, &account, command));
        match result {
            Ok(()) => {}
            Err(SshError::Disconnected) => return,
            Err(error) => {
                eprintln!("bsshd: session channel failed: {error:?}");
                return;
            }
        }
    }
}

fn run_channel(session: &mut ServerSession, account: &Account, command: SessionCommand) -> Result<(), SshError> {
    let program = match command {
        SessionCommand::Sftp => PipedProgram::Sftp {
            target_first: session.client_version().starts_with("SSH-2.0-OpenSSH_"),
        },
        SessionCommand::Exec(ref command) if session.pty_term().is_none() => PipedProgram::Exec(command),
        command => {
            let exec_command = match command {
                SessionCommand::Exec(command) => Some(command),
                _ => None,
            };
            let shell_status = run_shell(session, account, exec_command)?;
            return session.finish(shell_status);
        }
    };
    let status = run_piped_program(session, account, program)?;
    session.finish(status)
}

fn run_shell(session: &mut ServerSession, account: &Account, exec_command: Option<String>) -> Result<i32, SshError> {
    let fd = session.io().fd();
    let term = session.pty_term();
    let pty_requested = term.is_some();
    // Shell input and output are separate pipes without a requested PTY.
    let mut child_pipe_fds = None;
    let (master_fd, input_fd, slave_path) = if pty_requested {
        match pty::openpty() {
            Ok((master, path)) => {
                if let Err(error) = set_pty_size(master, session.pty_size()) {
                    eprintln!("bsshd: initial PTY size: {error}");
                    let _ = io::close(master);
                    return Ok(1);
                }
                (master, master, path)
            }
            Err(e) => {
                eprintln!("bsshd: openpty() failed: {:?}", e);
                return Ok(1);
            }
        }
    } else {
        let (input_read, input_write) = match io::pipe() {
            Ok(pipe) => pipe,
            Err(_) => return Ok(1),
        };
        let (output_read, output_write) = match io::pipe() {
            Ok(pipe) => pipe,
            Err(_) => {
                let _ = io::close(input_read);
                let _ = io::close(input_write);
                return Ok(1);
            }
        };
        child_pipe_fds = Some((input_read, output_write));
        (output_read, input_write, [0u8; 32])
    };

    // Fork a child for the shell
    match process::fork() {
        Ok(process::ForkResult::Child) => {
            // Child: close master, set up slave as stdin/stdout/stderr
            let _ = io::close(fd);
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
                // Init ignores HUP while detaching its console. A new login
                // must terminate when its own controlling terminal hangs up.
                if signal::sigaction(signal::SIGHUP, Some(&signal::Sigaction::default_action()), None).is_err() {
                    process::exit(1);
                }
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
                    || session::grant_terminal(slave_fd.raw() as i32, account).is_err()
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
            let environment = match enter_account(account, term.as_deref()) {
                Ok(environment) => environment,
                Err(why) => {
                    eprintln!("bsshd: session for '{}' refused: {}", account.name, why);
                    process::exit(1);
                }
            };
            let mut envp: Vec<*const u8> = environment.iter().map(|entry| entry.as_ptr().cast()).collect();
            envp.push(core::ptr::null());
            let shell = c_string(&account.shell);
            let shell_name = account.shell.rsplit('/').next().unwrap_or("sh");
            let arguments = match &exec_command {
                Some(command) => vec![c_string(shell_name), c_string("-c"), c_string(command)],
                None => vec![c_string(&format!("-{}", shell_name))],
            };
            let mut argv: Vec<*const u8> = arguments.iter().map(|arg| arg.as_ptr()).collect();
            argv.push(core::ptr::null());
            let _ = process::execve(&shell, argv.as_ptr(), envp.as_ptr());
            eprintln!("bsshd: cannot run {}", account.shell);
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(child_pid)) => {
            // Parent: shuttle data between SSH channel and PTY master
            println!(
                "bsshd: shell started (pid {}) for user '{}'",
                child_pid.raw(),
                account.name
            );
            if let Some((input_read, output_write)) = child_pipe_fds {
                let _ = io::close(input_read);
                let _ = io::close(output_write);
            }
            let (input_closed, connected) = data_shuttle(session, master_fd, input_fd, pty_requested);
            println!("bsshd: session ended for user '{}'", account.name);

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
            if connected {
                wait_for_shell(session, child_pid.raw() as i32)
            } else {
                Err(SshError::Disconnected)
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
            Ok(1)
        }
    }
}

/// Re-executing bsshd clears the address space before SFTP handles any
/// requests. The transport and host key stay in the SSH connection process.
fn run_sftp_worker(target_first: bool) -> i32 {
    // SAFETY: geteuid has no arguments and only reads the current identity.
    if unsafe { geteuid() } == 0 || session::real_uid() == 0 {
        return 1;
    }
    let mut stream = SftpStdio {
        input: std::io::stdin().lock(),
        output: std::io::stdout().lock(),
    };
    match bsshd_sftp::serve(&mut stream, target_first) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

struct SftpStdio<'a> {
    input: std::io::StdinLock<'a>,
    output: std::io::StdoutLock<'a>,
}

impl std::io::Read for SftpStdio<'_> {
    fn read(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(data)
    }
}

impl std::io::Write for SftpStdio<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.output.write(data)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

enum PipedProgram<'a> {
    Exec(&'a str),
    Sftp { target_first: bool },
}

fn run_piped_program(
    session: &mut ServerSession,
    account: &Account,
    program: PipedProgram<'_>,
) -> Result<i32, SshError> {
    let sftp = matches!(program, PipedProgram::Sftp { .. });
    let (path, arguments) = match program {
        PipedProgram::Exec(command) => {
            println!("bsshd: exec request");
            let shell_name = account.shell.rsplit('/').next().unwrap_or(&account.shell);
            (account.shell.as_str(), vec![c_string(shell_name), c_string("-c"), c_string(command)])
        }
        PipedProgram::Sftp { target_first } => {
            let flag = if target_first { "--sftp-server-openssh" } else { "--sftp-server" };
            ("/usr/sbin/bsshd", vec![c_string("bsshd"), c_string(flag)])
        }
    };

    let (read_fd, write_fd) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(e) => {
            let msg = format!("bsshd: pipe failed: {:?}\n", e);
            let _ = if sftp { session.send_stderr(msg.as_bytes()) } else { session.send_data(msg.as_bytes()) };
            return Ok(1);
        }
    };

    let (input_read, input_write) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(_) => {
            let _ = io::close(read_fd);
            let _ = io::close(write_fd);
            return Ok(1);
        }
    };
    let (error_read, error_write) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(_) => {
            for fd in [read_fd, write_fd, input_read, input_write] { let _ = io::close(fd); }
            return Ok(1);
        }
    };
    // Backpressure on command input must not prevent draining its output.
    if io::fcntl_setfl(input_write, io::status_flags::O_NONBLOCK).is_err() {
        for fd in [read_fd, write_fd, error_read, error_write, input_read, input_write] { let _ = io::close(fd); }
        return Ok(1);
    }

    match process::fork() {
        Ok(process::ForkResult::Child) => {
            let _ = io::close(session.io().fd());
            let _ = io::close(read_fd);
            let _ = io::close(error_read);
            let _ = io::close(input_write);
            for (fd, stdio) in [(input_read, 0), (write_fd, 1), (error_write, 2)] {
                if io::dup2(fd, Fd::from_raw(stdio)).is_err() {
                    process::exit(1);
                }
                if fd.raw() > 2 {
                    let _ = io::close(fd);
                }
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
            let path_c = c_string(path);
            let mut argv: Vec<*const u8> = arguments.iter().map(|arg| arg.as_ptr()).collect();
            argv.push(core::ptr::null());
            let _ = process::execve(&path_c, argv.as_ptr(), envp.as_ptr());
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(child_pid)) => {
            let _ = io::close(write_fd);
            let _ = io::close(error_write);
            let _ = io::close(input_read);
            exec_shuttle(session, [read_fd, error_read], input_write).map_err(|why| {
                eprintln!("bsshd: piped program relay failed: {}", why);
                SshError::Io
            })?;
            wait_for_shell(session, child_pid.raw() as i32)
        }
        Err(e) => {
            let _ = io::close(error_read);
            let _ = io::close(error_write);
            let _ = io::close(input_read);
            let _ = io::close(input_write);
            let _ = io::close(read_fd);
            let _ = io::close(write_fd);
            let msg = format!("bsshd: fork failed: {:?}\n", e);
            let _ = if sftp { session.send_stderr(msg.as_bytes()) } else { session.send_data(msg.as_bytes()) };
            Ok(1)
        }
    }
}

/// Program output read per channel packet: up to 32 KiB, still capped by
/// the peer's window and maximum packet size.
const RELAY_CHUNK: usize = 32 * 1024;

/// RFC 4254 section 5.3: input EOF closes only stdin; output keeps flowing.
fn exec_shuttle(session: &mut ServerSession, outputs: [Fd; 2], input: Fd) -> Result<(), String> {
    use io::poll_events::{POLLERR, POLLHUP, POLLIN, POLLOUT};
    let mut buf = vec![0u8; RELAY_CHUNK];
    let mut pending = Vec::new();
    let mut offset = 0;
    let mut input_open = true;
    let mut output_open = [true; 2];
    let result = (|| {
        loop {
            let capacity = session.send_capacity().min(RELAY_CHUNK);
            // A packet already taken off the socket will not wake poll.
            let buffered = session.io().packet_ready();
            let mut fds = [
                io::PollFd::new(if capacity > 0 && output_open[0] { outputs[0] } else { Fd::from_raw(u64::MAX) }, POLLIN),
                io::PollFd::new(session.io().fd(), POLLIN),
                io::PollFd::new(if input_open { input } else { Fd::from_raw(u64::MAX) },
                               if pending.is_empty() { 0 } else { POLLOUT }),
                io::PollFd::new(if capacity > 0 && output_open[1] { outputs[1] } else { Fd::from_raw(u64::MAX) }, POLLIN),
            ];
            match io::poll(&mut fds, if buffered { 0 } else { 100 }) {
                Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINTR)) => continue,
                Err(e) => return Err(format!("poll: {:?}", e)),
                Ok(_) => {}
            }
            if buffered {
                fds[1].revents |= POLLIN;
            }
            for (index, slot) in [(0, 0), (1, 3)] {
                let capacity = session.send_capacity().min(RELAY_CHUNK);
                if capacity == 0 || fds[slot].revents & (POLLIN | POLLHUP) == 0 { continue; }
                match io::read(outputs[index], &mut buf[..capacity]) {
                    Ok(0) => output_open[index] = false,
                    Ok(n) => {
                        let sent = if index == 0 { session.send_data(&buf[..n]) } else { session.send_stderr(&buf[..n]) };
                        sent.map_err(|e| format!("send output: {:?}", e))?;
                    }
                    Err(e) => return Err(format!("read command output: {:?}", e)),
                }
            }
            if output_open == [false; 2] { return Ok(()); }
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
                    Err(SshError::Disconnected) if session.channel_closed() => return Ok(()),
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
    // Close readers first so a publisher can distinguish an aborted relay
    // from a normal channel EOF, even after receiving the advertised length.
    for fd in outputs { let _ = io::close(fd); }
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

fn set_pty_size(master: Fd, (cols, rows): (u32, u32)) -> Result<(), Error> {
    termios::set_winsize(master, &termios::Winsize {
        ws_col: cols.min(u32::from(u16::MAX)) as u16,
        ws_row: rows.min(u32::from(u16::MAX)) as u16,
        ..Default::default()
    })
}

/// Relay a shell's input and output. Return which pipe end we closed and
/// whether the channel is still connected for an exit status.
fn data_shuttle(session: &mut ServerSession, master_fd: Fd, input_fd: Fd, has_pty: bool) -> (bool, bool) {
    use io::poll_events::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, POLLRDHUP};
    let mut input_closed = false;
    let result = (|| {
        let flags = io::fcntl_getfl(input_fd).map_err(|_| SshError::Io)? as i32;
        io::fcntl_setfl(input_fd, flags | io::status_flags::O_NONBLOCK).map_err(|_| SshError::Io)?;
        session.io().start_relay().map_err(|_| SshError::Io)?;
        let ssh_fd = session.io().fd();
        let unused = Fd::from_raw(u64::MAX);
        let mut size = session.pty_size();
        let mut pending = std::collections::VecDeque::new();
        let mut eof_sent = false;
        let mut pending_eof = false;
        let mut output_done = false;
        let mut bytes = vec![0u8; RELAY_CHUNK];

        loop {
            let queue_ready = session.io().relay_can_queue();
            let output_pending = session.io().relay_output_pending();
            if output_done && !output_pending { break; }
            let capacity = if queue_ready { session.send_capacity().min(RELAY_CHUNK) } else { 0 };
            // A packet already taken off the socket will not wake poll.
            let buffered = queue_ready && !session.channel_closed() && session.io().packet_ready();
            let output = if capacity > 0 && !output_done { master_fd } else { unused };
            let input = if !session.channel_closed() && !input_closed && !pending.is_empty() { input_fd } else { unused };
            let socket_events = (if queue_ready && !session.channel_closed() { POLLIN } else { POLLRDHUP })
                | (if output_pending { POLLOUT } else { 0 });
            let mut fds = [
                io::PollFd::new(output, POLLIN),
                io::PollFd::new(ssh_fd, socket_events),
                io::PollFd::new(input, POLLOUT),
            ];
            match io::poll(&mut fds, if buffered { 0 } else { 100 }) {
                Err(Error::Os(Errno::EINTR)) => continue,
                Err(_) => return Err(SshError::Io),
                Ok(_) => {}
            }

            let ssh_events = fds[1].revents | if buffered { POLLIN } else { 0 };
            if ssh_events & (POLLERR | POLLHUP | POLLNVAL | POLLRDHUP) != 0 {
                return Err(SshError::Disconnected);
            }
            if ssh_events & POLLOUT != 0 {
                session.io().flush_relay().map_err(|_| SshError::Io)?;
            }
            let mut new_input = false;
            if ssh_events & POLLIN != 0 {
                match session.try_recv_data_uncredited() {
                    Ok(Some(data)) if input_closed => session.consume_input(data.len())?,
                    Ok(Some(data)) if !data.is_empty() => {
                        pending.extend(data);
                        new_input = true;
                    }
                    Ok(_) => {}
                    Err(SshError::Disconnected) if session.channel_closed() => {
                        output_done = true;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
                if has_pty && session.pty_size() != size {
                    size = session.pty_size();
                    set_pty_size(master_fd, size).map_err(|_| SshError::Io)?;
                }
            }
            if session.input_eof() && !eof_sent && !input_closed {
                if has_pty {
                    // Deliver the PTY's existing end-of-input convention
                    // behind any channel data already accepted.
                    pending.push_back(4);
                    pending_eof = true;
                    new_input = true;
                    eof_sent = true;
                } else if pending.is_empty() {
                    let _ = io::close(input_fd);
                    input_closed = true;
                    eof_sent = true;
                }
            }

            if new_input || fds[2].revents & (POLLOUT | POLLERR | POLLHUP) != 0 {
                match io::write(input_fd, pending.as_slices().0) {
                    Ok(0) => return Err(SshError::Io),
                    Ok(n) => {
                        pending.drain(..n);
                        let local_eof = pending_eof && pending.is_empty();
                        session.consume_input(n - usize::from(local_eof))?;
                        pending_eof &= !local_eof;
                    }
                    Err(Error::Os(Errno::EAGAIN | Errno::EINTR)) => {}
                    // A PTY master EIO means the slave went away. Keep
                    // draining its last output below.
                    Err(Error::Os(Errno::EIO)) if has_pty => {
                        session.consume_input(pending.len() - usize::from(pending_eof))?;
                        pending.clear();
                        input_closed = true;
                    }
                    Err(_) if !has_pty => {
                        let _ = io::close(input_fd);
                        input_closed = true;
                        session.consume_input(pending.len())?;
                        pending.clear();
                    }
                    Err(_) => return Err(SshError::Io),
                }
            }

            if fds[0].revents & POLLIN != 0 {
                match io::read(master_fd, &mut bytes[..capacity]) {
                    Ok(0) | Err(Error::Os(Errno::EIO)) => output_done = true,
                    Ok(n) => session.send_data(&bytes[..n])?,
                    Err(Error::Os(Errno::EAGAIN | Errno::EINTR)) => {}
                    Err(_) => return Err(SshError::Io),
                }
            } else if fds[0].revents & POLLHUP != 0 {
                output_done = true;
            } else if fds[0].revents & (POLLERR | POLLNVAL) != 0 {
                return Err(SshError::Io);
            }
        }
        session.io().finish_relay().map_err(|_| SshError::Io)
    })();
    if result.is_err() && session.io().relay_output_pending() {
        // An explicit channel CLOSE can have queued an acknowledgment.
        let _ = session.io().flush_relay();
    }
    (input_closed, result.is_ok())
}

/// A shell can close its terminal descriptors before it exits. Keep
/// answering connection requests while waiting for its status.
fn wait_for_shell(session: &mut ServerSession, pid: i32) -> Result<i32, SshError> {
    use io::poll_events::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, POLLRDHUP};
    session.io().start_relay().map_err(|_| SshError::Io)?;
    let mut status = 0;
    let mut exit = None;
    loop {
        if exit.is_none() {
            match process::waitpid(pid, &mut status, process::WNOHANG) {
                Ok(child) if child.raw() != 0 => {
                    exit = Some(if process::wifexited(status) {
                        process::wexitstatus(status)
                    } else if process::wifsignaled(status) {
                        128 + process::wtermsig(status)
                    } else { 1 });
                }
                Ok(_) | Err(Error::Os(Errno::EINTR)) => {}
                Err(_) => return Err(SshError::Io),
            }
        }
        let pending = session.io().relay_output_pending();
        let closed = session.channel_closed();
        if !pending && (exit.is_some() || closed) {
            session.io().finish_relay().map_err(|_| SshError::Io)?;
            if let Some(status) = exit {
                return Ok(status);
            }
            // The peer abandoned this channel. The unreaped PID still refers
            // to this channel's own child. Release it before accepting another.
            let _ = signal::kill(pid, signal::SIGKILL);
            loop {
                match process::waitpid(pid, &mut status, 0) {
                    Err(Error::Os(Errno::EINTR)) => continue,
                    Err(_) => return Err(SshError::Io),
                    Ok(_) => return Ok(1),
                }
            }
        }
        let readable = !closed && session.io().relay_can_queue();
        let buffered = readable && session.io().packet_ready();
        let events = (if readable { POLLIN } else { POLLRDHUP })
            | (if pending { POLLOUT } else { 0 });
        let mut fds = [io::PollFd::new(session.io().fd(), events)];
        match io::poll(&mut fds, if buffered { 0 } else { 100 }) {
            Err(Error::Os(Errno::EINTR)) => continue,
            Err(_) => return Err(SshError::Io),
            Ok(_) => {}
        }
        let events = fds[0].revents | if buffered { POLLIN } else { 0 };
        if events & (POLLERR | POLLHUP | POLLNVAL | POLLRDHUP) != 0 {
            return Err(SshError::Disconnected);
        }
        if events & POLLOUT != 0 {
            session.io().flush_relay().map_err(|_| SshError::Io)?;
        }
        if events & POLLIN != 0 {
            match session.try_recv_data_uncredited() {
                Ok(Some(data)) => session.consume_input(data.len())?,
                Ok(None) => {}
                Err(SshError::Disconnected) if session.channel_closed() => {}
                Err(error) => return Err(error),
            }
        }
    }
}
