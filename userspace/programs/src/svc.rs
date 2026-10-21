//! svc - report and control the services init supervises.
//!
//!   svc [status [NAME...]]       each service's state, from /run/breenix/services
//!   svc start|stop|restart NAME  ask init; root, or the account the service runs as
//!   svc reload                   init reads /etc/breenix/services again (root)
//!
//! `status` exits 0 when each named service is running (or ran to completion),
//! 3 when one is not, and 4 for an unknown name. The other commands exit 0 when
//! init answers `ok` and 1 otherwise; `stop` and `restart` return once the
//! service has stopped or started again.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use libbreenix::services::{self, Status};

fn usage() -> ! {
    eprintln!("usage: svc [status [NAME...]] | svc start|stop|restart NAME | svc reload");
    std::process::exit(2);
}

fn now_ms() -> u64 {
    libbreenix::time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0)
}

fn seconds(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}

/// What a service is doing now, in words.
fn note(status: &Status, supervisor: &services::Supervisor, all: &[Status], now: u64) -> String {
    let since = seconds(now.saturating_sub(status.since_ms));
    match status.state.as_str() {
        "running" => format!("up {}", since),
        "backoff" | "crash-loop" => format!("restart in {}", seconds(status.next_ms.saturating_sub(now))),
        "waiting" => {
            let unmet: Vec<&str> = status
                .needs
                .iter()
                .map(String::as_str)
                .filter(|need| match *need {
                    "entropy" => supervisor.entropy_ms == 0,
                    "network-up" => supervisor.network_ms == 0,
                    name => !all.iter().any(|s| s.name == name && matches!(s.state.as_str(), "running" | "done")),
                })
                .collect();
            if unmet.is_empty() { "starting".into() } else { format!("needs {}", unmet.join(", ")) }
        }
        _ => format!("for {}", since),
    }
}

fn status(names: &[String]) -> i32 {
    let text = match std::fs::read_to_string(services::STATE) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("svc: {}: {} (init publishes it once it supervises services)", services::STATE, e);
            return 1;
        }
    };
    let (supervisor, all) = match services::parse_state(&text) {
        Ok(state) => state,
        Err(why) => {
            eprintln!("svc: {}: {}", services::STATE, why);
            return 1;
        }
    };
    let now = now_ms();
    let shown: Vec<&Status> = if names.is_empty() { all.iter().collect() } else { all.iter().filter(|s| names.contains(&s.name)).collect() };
    let mut code = 0;
    for name in names {
        if !all.iter().any(|s| &s.name == name) {
            eprintln!("svc: no service '{}'", name);
            code = 4;
        }
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{:<20} {:<10} {:>6} {:>6}  {:<10} {:<8} {}", "SERVICE", "STATE", "PID", "STARTS", "LAST-EXIT", "USER", "NOTE");
    for s in &shown {
        let pid = if s.pid == 0 { "-".to_string() } else { s.pid.to_string() };
        let last = if s.last_exit.is_empty() { "-".to_string() } else { s.last_exit.replace(':', " ") };
        let _ = writeln!(
            out,
            "{:<20} {:<10} {:>6} {:>6}  {:<10} {:<8} {}",
            s.name, s.state, pid, s.starts, last, s.user, note(s, &supervisor, &all, now)
        );
        if code == 0 && !names.is_empty() && !matches!(s.state.as_str(), "running" | "done") {
            code = 3;
        }
    }
    code
}

fn request(line: &str) -> i32 {
    let mut stream = match UnixStream::connect(services::SOCKET) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("svc: {}: {} (is init supervising services?)", services::SOCKET, e);
            return 1;
        }
    };
    if let Err(e) = stream.write_all(format!("{}\n", line).as_bytes()) {
        eprintln!("svc: {}: {}", services::SOCKET, e);
        return 1;
    }
    let mut reply = String::new();
    if let Err(e) = BufReader::new(stream).read_line(&mut reply) {
        eprintln!("svc: {}: {}", services::SOCKET, e);
        return 1;
    }
    let reply = reply.trim_end();
    match reply.strip_prefix("ok ") {
        Some(text) => {
            println!("svc: {}", text);
            0
        }
        None => {
            eprintln!("svc: {}", reply.strip_prefix("error ").unwrap_or(if reply.is_empty() { "init closed the request" } else { reply }));
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => status(&[]),
        ["status", ..] => status(&args[1..]),
        [verb @ ("start" | "stop" | "restart"), name] if services::valid_name(name) => request(&format!("{} {}", verb, name)),
        ["reload"] => request("reload"),
        _ => usage(),
    };
    std::process::exit(code);
}
