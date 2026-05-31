//! BSSH — Breenix SSH client
//!
//! Connects to a remote SSH server and opens an interactive shell session.
//!
//! Usage: bssh [user@]<host> [port] [username] [password|--publickey|--publickey-wrong] [--smoke] [--exec command]
//!   Default port: 22
//!   Default username: root
//!   Default password: breenix

use libbreenix::fs;
use libbreenix::io;
use libbreenix::socket::{SockAddrIn, TcpStream};
use libbreenix::ssh::keys::ServerHostKeyInfo;
use libbreenix::ssh::transport::{ClientAuthMethod, ClientSession};
use libbreenix::termios;
use libbreenix::types::Fd;

const DEFAULT_KNOWN_HOSTS: &str = "/tmp/bssh_known_hosts";

enum AuthChoice {
    Password(String),
    PublicKey { wrong_key: bool },
}

struct Options {
    host: String,
    port: u16,
    username: String,
    auth_choice: AuthChoice,
    smoke: bool,
    exec_command: Option<String>,
    known_hosts_path: String,
    host_key_alias: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let opts = match parse_options(&args) {
        Ok(opts) => opts,
        Err(message) => {
            eprintln!("{}", message);
            usage();
            std::process::exit(1);
        }
    };

    // Parse host as IPv4 address
    let addr_bytes = match parse_ipv4(&opts.host) {
        Some(b) => b,
        None => {
            // Try DNS resolution
            match libbreenix::dns::resolve_auto(&opts.host) {
                Ok(result) => result.addr,
                Err(e) => {
                    eprintln!("bssh: cannot resolve '{}': {:?}", opts.host, e);
                    std::process::exit(1);
                }
            }
        }
    };

    println!(
        "bssh: connecting to {}.{}.{}.{}:{}",
        addr_bytes[0], addr_bytes[1], addr_bytes[2], addr_bytes[3], opts.port
    );

    // Connect
    let addr = SockAddrIn::new(addr_bytes, opts.port);
    let stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bssh: connection failed: {:?}", e);
            std::process::exit(1);
        }
    };

    let fd = stream.into_raw_fd();
    let mut session = ClientSession::new(fd);
    let known_host_id = known_host_id(&opts.host, opts.port, opts.host_key_alias.as_deref());

    // SSH handshake + auth
    let auth_method = match &opts.auth_choice {
        AuthChoice::Password(password) => ClientAuthMethod::Password(password),
        AuthChoice::PublicKey { wrong_key } => ClientAuthMethod::PublicKey {
            wrong_key: *wrong_key,
        },
    };
    if let Err(e) =
        session.handshake_with_auth_and_host_key(&opts.username, auth_method, |host_key| {
            verify_or_pin_known_host(&opts.known_hosts_path, &known_host_id, host_key)
        })
    {
        if matches!(e, libbreenix::ssh::SshError::Auth) {
            eprintln!("bssh: authentication failed");
        } else {
            eprintln!("bssh: handshake failed: {:?}", e);
        }
        std::process::exit(1);
    }
    println!("bssh: authenticated as '{}'", opts.username);

    if let Some(command) = opts.exec_command {
        if let Err(e) = session.open_exec(&command) {
            eprintln!("bssh: exec request failed: {:?}", e);
            std::process::exit(1);
        }

        println!("BSSH_EXEC_BEGIN host={} command={}", opts.host, command);
        let rc = drain_exec_output(&mut session);
        println!("BSSH_EXEC_END host={} rc={}", opts.host, rc);
        session.close();
        std::process::exit(rc);
    }

    // Open channel + PTY + shell
    if let Err(e) = session.open_shell() {
        eprintln!("bssh: shell request failed: {:?}", e);
        std::process::exit(1);
    }
    println!("bssh: shell opened");
    if opts.smoke {
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

fn usage() {
    eprintln!(
        "Usage: bssh [user@]<host> [port] [username] [password|--publickey|--publickey-wrong] [--user name] [--password password] [--known-hosts path] [--host-key-alias id] [--smoke] [--exec command]"
    );
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    if args.len() < 2 {
        return Err("bssh: missing host".to_string());
    }

    let (host, username_from_host) = match args[1].split_once('@') {
        Some((user, host)) if !user.is_empty() && !host.is_empty() => {
            (host.to_string(), Some(user.to_string()))
        }
        _ => (args[1].clone(), None),
    };
    let mut idx = 2;

    let port = if let Some(arg) = args.get(idx) {
        if !arg.starts_with("--") {
            if let Ok(port) = arg.parse::<u16>() {
                idx += 1;
                port
            } else {
                22
            }
        } else {
            22
        }
    } else {
        22
    };

    let username = if let Some(arg) = args.get(idx) {
        if !arg.starts_with("--") {
            idx += 1;
            arg.clone()
        } else {
            username_from_host.unwrap_or_else(|| "root".to_string())
        }
    } else {
        username_from_host.unwrap_or_else(|| "root".to_string())
    };

    let mut username = username;
    let mut auth_choice = AuthChoice::Password("breenix".to_string());
    if let Some(arg) = args.get(idx) {
        if arg == "--publickey" || arg == "publickey" {
            auth_choice = AuthChoice::PublicKey { wrong_key: false };
            idx += 1;
        } else if arg == "--publickey-wrong" || arg == "publickey-wrong" {
            auth_choice = AuthChoice::PublicKey { wrong_key: true };
            idx += 1;
        } else if !arg.starts_with("--") {
            auth_choice = AuthChoice::Password(arg.clone());
            idx += 1;
        }
    }

    let mut smoke = false;
    let mut exec_command = None;
    let mut known_hosts_path = DEFAULT_KNOWN_HOSTS.to_string();
    let mut host_key_alias = None;

    while idx < args.len() {
        match args[idx].as_str() {
            "--smoke" => {
                smoke = true;
                idx += 1;
            }
            "--publickey" | "publickey" => {
                auth_choice = AuthChoice::PublicKey { wrong_key: false };
                idx += 1;
            }
            "--publickey-wrong" | "publickey-wrong" => {
                auth_choice = AuthChoice::PublicKey { wrong_key: true };
                idx += 1;
            }
            "--user" => {
                let user = args
                    .get(idx + 1)
                    .ok_or_else(|| "bssh: --user requires a username".to_string())?;
                username = user.clone();
                idx += 2;
            }
            "--password" => {
                let password = args
                    .get(idx + 1)
                    .ok_or_else(|| "bssh: --password requires a password".to_string())?;
                auth_choice = AuthChoice::Password(password.clone());
                idx += 2;
            }
            "--known-hosts" => {
                let path = args
                    .get(idx + 1)
                    .ok_or_else(|| "bssh: --known-hosts requires a path".to_string())?;
                known_hosts_path = path.clone();
                idx += 2;
            }
            "--host-key-alias" => {
                let alias = args
                    .get(idx + 1)
                    .ok_or_else(|| "bssh: --host-key-alias requires an id".to_string())?;
                host_key_alias = Some(alias.clone());
                idx += 2;
            }
            "--exec" => {
                let parts = args
                    .get(idx + 1..)
                    .ok_or_else(|| "bssh: --exec requires a command".to_string())?;
                if parts.is_empty() {
                    return Err("bssh: --exec requires a command".to_string());
                }
                exec_command = Some(parts.join(" "));
                break;
            }
            other => return Err(format!("bssh: unknown option '{}'", other)),
        }
    }

    Ok(Options {
        host,
        port,
        username,
        auth_choice,
        smoke,
        exec_command,
        known_hosts_path,
        host_key_alias,
    })
}

fn known_host_id(host: &str, port: u16, alias: Option<&str>) -> String {
    if let Some(alias) = alias {
        return alias.to_string();
    }

    format!("[{}]:{}", host, port)
}

fn verify_or_pin_known_host(
    path: &str,
    host_id: &str,
    host_key: &ServerHostKeyInfo,
) -> Result<(), libbreenix::ssh::SshError> {
    let fingerprint = fingerprint_hex(&host_key.fingerprint);

    if let Some(entry) = find_known_host(path, host_id) {
        if entry.key_type == host_key.key_type && entry.fingerprint == host_key.fingerprint {
            return Ok(());
        }

        eprintln!(
            "bssh: host key verification failed for {}: expected {} SHA256:{}, got {} SHA256:{}",
            host_id,
            entry.key_type,
            fingerprint_hex(&entry.fingerprint),
            host_key.key_type,
            fingerprint
        );
        return Err(libbreenix::ssh::SshError::Protocol(
            "host key verification failed",
        ));
    }

    let line = format!(
        "{} {} SHA256:{} {}\n",
        host_id, host_key.key_type, fingerprint, host_key.algorithm
    );
    let fd = match fs::open_with_mode(path, fs::O_WRONLY | fs::O_CREAT | fs::O_APPEND, 0o644) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("bssh: failed to open known_hosts '{}': {:?}", path, e);
            return Err(libbreenix::ssh::SshError::Io);
        }
    };

    let result = fs::write(fd, line.as_bytes());
    let _ = fs::close(fd);
    if let Err(e) = result {
        eprintln!("bssh: failed to write known_hosts '{}': {:?}", path, e);
        return Err(libbreenix::ssh::SshError::Io);
    }

    println!(
        "bssh: pinned host key {} algorithm={} fingerprint=SHA256:{}",
        host_id, host_key.algorithm, fingerprint
    );
    Ok(())
}

struct KnownHostEntry {
    key_type: String,
    fingerprint: [u8; 32],
}

fn find_known_host(path: &str, host_id: &str) -> Option<KnownHostEntry> {
    let content = read_file(path)?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let hosts = parts.next()?;
        let key_type = parts.next()?;
        let fingerprint = parts.next()?;

        if hosts.split(',').any(|candidate| candidate == host_id) {
            let fingerprint = parse_fingerprint(fingerprint)?;
            return Some(KnownHostEntry {
                key_type: key_type.to_string(),
                fingerprint,
            });
        }
    }

    None
}

fn read_file(path: &str) -> Option<String> {
    let fd = fs::open(path, fs::O_RDONLY).ok()?;
    let mut bytes = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        match fs::read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(_) => {
                let _ = fs::close(fd);
                return None;
            }
        }
    }
    let _ = fs::close(fd);
    String::from_utf8(bytes).ok()
}

fn parse_fingerprint(value: &str) -> Option<[u8; 32]> {
    let hex = value.strip_prefix("SHA256:")?;
    if hex.len() != 64 {
        return None;
    }

    let mut out = [0u8; 32];
    for i in 0..32 {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        out[i] = byte;
    }
    Some(out)
}

fn fingerprint_hex(fingerprint: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in fingerprint {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
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
