//! kiosk_test: the breengel suite's test compositor (`docs/gui/breengel.md` section 24).
//!
//! It claims the compositor and prints `ready <epoch>`, or `claim-failed <errno>` and exits 1.
//! Then it reads one command per line on standard input and answers each with one line on
//! standard output: `ok ...` or `err <errno>`.
//!
//!   place <id> <x> <y> <z> <flags>   SET_GEOMETRY
//!   event <id> <640 hex digits>      WRITE_EVENTS of one 320-byte event -> ok <queued>
//!   composite                        COMPOSITE -> ok <serial> <pixels>
//!   auto <0|1>                       compose after every commit it sees (on at start)
//!   release                          RELEASE_COMPOSITOR
//!   quit                             exit 0
//!
//! With `auto` on it lists the windows every 2 ms and composes when any registered window's
//! commit id has changed since its last composite.

use std::collections::BTreeMap;

use libbreenix::io::{self, poll_events::POLLIN, PollFd};
use libbreenix::types::Fd;
use libbreenix::window::{self, abi};

fn say(line: &str) {
    let mut out = String::from(line);
    out.push('\n');
    let _ = io::write(Fd::STDOUT, out.as_bytes());
}

fn errno(e: libbreenix::error::Error) -> i64 {
    match e {
        libbreenix::error::Error::Os(errno) => errno as i64,
    }
}

fn reply(result: Result<String, libbreenix::error::Error>) {
    match result {
        Ok(rest) if rest.is_empty() => say("ok"),
        Ok(rest) => say(&format!("ok {rest}")),
        Err(e) => say(&format!("err {}", errno(e))),
    }
}

fn hex_event(text: &str) -> Option<abi::WindowEvent> {
    let size = core::mem::size_of::<abi::WindowEvent>();
    if text.len() != size * 2 {
        return None;
    }
    let mut bytes = vec![0u8; size];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    // SAFETY: WindowEvent is plain data and `bytes` holds exactly one.
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const abi::WindowEvent) })
}

fn command(line: &str, auto: &mut bool) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    let num = |i: usize| words.get(i).and_then(|w| w.parse::<i64>().ok());
    match words.first().copied() {
        Some("place") => match (num(1), num(2), num(3), num(4), num(5)) {
            (Some(id), Some(x), Some(y), Some(z), Some(flags)) => {
                reply(window::set_geometry(id as u64, x as i32, y as i32, z as u32, flags as u32).map(|_| String::new()))
            }
            _ => say("err 22"),
        },
        Some("event") => match (num(1), words.get(2).and_then(|t| hex_event(t))) {
            (Some(id), Some(e)) => reply(window::write_events(id as u64, &[e]).map(|n| n.to_string())),
            _ => say("err 22"),
        },
        Some("composite") => reply(window::composite().map(|(serial, pixels)| format!("{serial} {pixels}"))),
        Some("auto") => {
            *auto = num(1) == Some(1);
            say("ok");
        }
        Some("release") => reply(window::release_compositor().map(|_| String::new())),
        Some("quit") => {
            say("ok");
            return false;
        }
        _ => say("err 22"),
    }
    true
}

/// Compose when any registered window has a commit not yet composed.
fn compose_new_commits(seen: &mut BTreeMap<u64, u64>) {
    let mut infos = vec![abi::WinInfo::default(); 64];
    let Ok(n) = window::list(&mut infos) else {
        return;
    };
    let mut changed = false;
    for info in infos.iter().take(n.min(infos.len())) {
        if seen.get(&info.id) != Some(&info.commit_id) {
            seen.insert(info.id, info.commit_id);
            changed = true;
        }
    }
    if changed {
        let _ = window::composite();
    }
}

fn main() {
    match window::claim_compositor() {
        Ok(epoch) => say(&format!("ready {epoch}")),
        Err(e) => {
            say(&format!("claim-failed {}", errno(e)));
            std::process::exit(1);
        }
    }
    let mut auto = true;
    let mut seen = BTreeMap::new();
    let mut pending = Vec::new();
    loop {
        let mut fds = [PollFd { fd: 0, events: POLLIN, revents: 0 }];
        let ready = io::poll(&mut fds, 2).unwrap_or(0);
        if ready > 0 && fds[0].revents != 0 {
            let mut buf = [0u8; 4096];
            match io::read(Fd::STDIN, &mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
            while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                if !command(&text, &mut auto) {
                    return;
                }
            }
        }
        if auto {
            compose_new_commits(&mut seen);
        }
    }
}
