//! BSSH — Breenix SSH client
//!
//! Connects to a remote SSH server and opens an interactive shell session.
//!
//! Usage: bssh <host> [port] [username] [password|--publickey|--publickey-wrong] [--smoke] [--exec command]
//!   Default port: 22
//!   Default username: root
//!   Default password: (prompted)

use libbreenix::io;
use libbreenix::socket::{SockAddrIn, TcpStream};
use libbreenix::ssh::transport::{ClientAuthMethod, ClientSession};
use libbreenix::termios;
use libbreenix::types::Fd;

enum AuthChoice<'a> {
    Password(&'a str),
    PublicKey { wrong_key: bool },
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: bssh <host> [port] [username] [password|--publickey|--publickey-wrong] [--smoke]");
        std::process::exit(1);
    }

    let host = &args[1];
    let port: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(22);
    let username = args.get(3).map(|s| s.as_str()).unwrap_or("root");
    let auth_arg = args.get(4).map(|s| s.as_str()).unwrap_or("breenix");
    let smoke = args.iter().any(|arg| arg == "--smoke");
    let exec_command = args.iter().position(|arg| arg == "--exec").and_then(|idx| {
        let parts = args.get(idx + 1..)?;
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    });
    let auth_choice = match auth_arg {
        "--publickey" | "publickey" => AuthChoice::PublicKey { wrong_key: false },
        "--publickey-wrong" | "publickey-wrong" => AuthChoice::PublicKey { wrong_key: true },
        password => AuthChoice::Password(password),
    };

    // Parse host as IPv4 address
    let addr_bytes = match parse_ipv4(host) {
        Some(b) => b,
        None => {
            // Try DNS resolution
            match libbreenix::dns::resolve_auto(host) {
                Ok(result) => result.addr,
                Err(e) => {
                    eprintln!("bssh: cannot resolve '{}': {:?}", host, e);
                    std::process::exit(1);
                }
            }
        }
    };

    println!(
        "bssh: connecting to {}.{}.{}.{}:{}",
        addr_bytes[0], addr_bytes[1], addr_bytes[2], addr_bytes[3], port
    );

    // Connect
    let addr = SockAddrIn::new(addr_bytes, port);
    let stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bssh: connection failed: {:?}", e);
            std::process::exit(1);
        }
    };

    let fd = stream.into_raw_fd();
    let mut session = ClientSession::new(fd);

    // SSH handshake + auth
    let auth_method = match auth_choice {
        AuthChoice::Password(password) => ClientAuthMethod::Password(password),
        AuthChoice::PublicKey { wrong_key } => ClientAuthMethod::PublicKey { wrong_key },
    };
    if let Err(e) = session.handshake_with_auth(username, auth_method) {
        if matches!(e, libbreenix::ssh::SshError::Auth) {
            eprintln!("bssh: authentication failed");
        } else {
            eprintln!("bssh: handshake failed: {:?}", e);
        }
        std::process::exit(1);
    }
    println!("bssh: authenticated as '{}'", username);

    if let Some(command) = exec_command {
        if let Err(e) = session.open_exec(&command) {
            eprintln!("bssh: exec request failed: {:?}", e);
            std::process::exit(1);
        }

        println!("BSSH_EXEC_BEGIN host={} command={}", host, command);
        let rc = drain_exec_output(&mut session);
        println!("BSSH_EXEC_END host={} rc={}", host, rc);
        session.close();
        std::process::exit(rc);
    }

    // Open channel + PTY + shell
    if let Err(e) = session.open_shell() {
        eprintln!("bssh: shell request failed: {:?}", e);
        std::process::exit(1);
    }
    println!("bssh: shell opened");
    if smoke {
        println!("bssh: smoke success");
        session.close();
        std::process::exit(0);
    }

    // Put local terminal in raw mode
    let mut old_termios: libbreenix::termios::Termios = unsafe { core::mem::zeroed() };
    let _ = termios::tcgetattr(Fd::from_raw(0), &mut old_termios);
    let mut raw = old_termios;
    termios::cfmakeraw(&mut raw);
    let _ = termios::tcsetattr(Fd::from_raw(0), 0, &raw);

    // Interactive loop: forward stdin to SSH, SSH to stdout
    let stdin_fd = Fd::from_raw(0);
    let stdout_fd = Fd::from_raw(1);

    // Get the SSH socket fd for polling
    let ssh_fd = session.io().fd();

    println!("\r\nbssh: connected. Press ~. to disconnect.\r\n");

    let mut last_was_tilde = false;
    loop {
        // Poll stdin and SSH socket
        let mut fds = [
            io::PollFd::new(stdin_fd, io::poll_events::POLLIN),
            io::PollFd::new(ssh_fd, io::poll_events::POLLIN),
        ];

        match io::poll(&mut fds, 100) {
            Ok(0) => continue,
            Err(_) => break,
            _ => {}
        }

        // Check stdin for user input
        if fds[0].revents & io::poll_events::POLLIN != 0 {
            let mut stdin_buf = [0u8; 256];
            match io::read(stdin_fd, &mut stdin_buf) {
                Ok(n) if n > 0 => {
                    // Check for escape sequence: ~.
                    for i in 0..n {
                        if last_was_tilde && stdin_buf[i] == b'.' {
                            println!("\r\nbssh: disconnected.");
                            session.close();
                            std::process::exit(0);
                        }
                        last_was_tilde =
                            stdin_buf[i] == b'~' && (i == 0 || stdin_buf[i - 1] == b'\r');
                    }

                    if session.send_data(&stdin_buf[..n]).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        // Check SSH socket for remote data
        if fds[1].revents & io::poll_events::POLLIN != 0 {
            match session.recv_data() {
                Ok(Some(data)) => {
                    let _ = io::write(stdout_fd, &data);
                }
                Ok(None) => {}
                Err(libbreenix::ssh::SshError::Disconnected) => {
                    println!("\r\nbssh: connection closed by remote.");
                    break;
                }
                Err(_) => break,
            }
        }

        // Check for hangup
        if fds[1].revents & io::poll_events::POLLHUP != 0 {
            println!("\r\nbssh: connection closed by remote.");
            break;
        }
    }

    session.close();
}

fn drain_exec_output(session: &mut ClientSession) -> i32 {
    let stdout_fd = Fd::from_raw(1);
    let ssh_fd = session.io().fd();
    let mut idle_ticks = 0u32;

    loop {
        let mut fds = [io::PollFd::new(ssh_fd, io::poll_events::POLLIN)];
        match io::poll(&mut fds, 100) {
            Ok(0) => {
                idle_ticks += 1;
                if idle_ticks >= 300 {
                    eprintln!("bssh: exec timed out waiting for remote output/close");
                    return 124;
                }
                continue;
            }
            Ok(_) => {
                idle_ticks = 0;
            }
            Err(_) => return 1,
        }

        if fds[0].revents & io::poll_events::POLLIN != 0 {
            match session.recv_data() {
                Ok(Some(data)) => {
                    let _ = io::write(stdout_fd, &data);
                }
                Ok(None) => {}
                Err(libbreenix::ssh::SshError::Disconnected) => {
                    return session.exit_status().unwrap_or(0);
                }
                Err(e) => {
                    eprintln!("bssh: exec receive failed: {:?}", e);
                    return 1;
                }
            }
        }

        if fds[0].revents & io::poll_events::POLLHUP != 0 {
            return session.exit_status().unwrap_or(0);
        }
    }
}

/// Parse an IPv4 address string into 4 bytes.
fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        bytes[i] = part.parse().ok()?;
    }
    Some(bytes)
}
