//! raw-window_test: a window client on the raw libbreenix calls, without Breengel
//! (`docs/gui/breengel.md` section 24).
//!
//! It prints `ready`, then reads one command per line on standard input and answers each with
//! one line: `ok ...`, `err <errno>`, or for `verify` a `mismatch ...` line.
//!
//!   create <w> <h> <buffers>      CREATE -> ok <id> <stride>
//!   register <id> <title>         REGISTER
//!   fill <id> <seed>              ACQUIRE, draw pattern <seed> over the whole buffer and COMMIT
//!                                 it with whole-window damage -> ok <commit_id> <pixel_gen>
//!                                 <buffer> <base> <replay_full>
//!   verify <id> <seed>            ACQUIRE and compare the buffer with pattern <seed> ->
//!                                 ok <buffer> <base> <replay_full> | mismatch <x> <y> <got> <want>;
//!                                 the buffer stays acquired for the next fill
//!   destroy <id>                  DESTROY
//!   quit                          exit 0
//!
//! Pattern <seed> at (x, y), as BGRX bytes: B = (7x + 13seed) mod 256, G = (5y + 29seed) mod 256,
//! R = (x xor y xor seed) mod 256, X = 0.

use std::collections::BTreeMap;

use libbreenix::io;
use libbreenix::types::Fd;
use libbreenix::window::{self, abi};

fn say(line: &str) {
    let _ = io::write(Fd::STDOUT, format!("{line}\n").as_bytes());
}

fn errno(e: libbreenix::error::Error) -> i64 {
    match e {
        libbreenix::error::Error::Os(errno) => errno as i64,
    }
}

fn pattern(seed: u32, x: u32, y: u32) -> [u8; 4] {
    [
        ((7 * x + 13 * seed) & 0xFF) as u8,
        ((5 * y + 29 * seed) & 0xFF) as u8,
        ((x ^ y ^ seed) & 0xFF) as u8,
        0,
    ]
}

struct Win {
    width: u32,
    height: u32,
    buffers: [u64; 3],
    /// A buffer acquired and not yet committed, with its base and replay flag.
    held: Option<(u32, u64, u32)>,
}

impl Win {
    fn pixels(&self, buffer: u32) -> &mut [u8] {
        let len = (self.width * self.height * 4) as usize;
        // SAFETY: the kernel mapped this buffer read-write for the owner when it was acquired,
        // `len` bytes long.
        unsafe { core::slice::from_raw_parts_mut(self.buffers[buffer as usize] as *mut u8, len) }
    }

    fn acquire(&mut self, id: u64) -> Result<(u32, u64, u32), libbreenix::error::Error> {
        if let Some(held) = self.held {
            return Ok(held);
        }
        let a = window::acquire(id, 5_000_000_000)?;
        let held = (a.buffer, a.base, a.replay_full);
        self.held = Some(held);
        Ok(held)
    }
}

fn run(line: &str, wins: &mut BTreeMap<u64, Win>) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    let num = |i: usize| words.get(i).and_then(|w| w.parse::<u64>().ok());
    match (words.first().copied(), num(1)) {
        (Some("create"), Some(w)) => match (num(2), num(3)) {
            (Some(h), Some(b)) => match window::create(w as u32, h as u32, b as u32, 0) {
                Ok(c) => {
                    wins.insert(c.id, Win { width: c.width, height: c.height, buffers: c.buffer, held: None });
                    say(&format!("ok {} {}", c.id, c.stride));
                }
                Err(e) => say(&format!("err {}", errno(e))),
            },
            _ => say("err 22"),
        },
        (Some("register"), Some(id)) => match window::register(id, words.get(2).unwrap_or(&"").as_bytes()) {
            Ok(_) => say("ok"),
            Err(e) => say(&format!("err {}", errno(e))),
        },
        (Some("fill"), Some(id)) => {
            let (Some(seed), Some(w)) = (num(2), wins.get_mut(&id)) else {
                say("err 22");
                return true;
            };
            let (buffer, base, full) = match w.acquire(id) {
                Ok(held) => held,
                Err(e) => {
                    say(&format!("err {}", errno(e)));
                    return true;
                }
            };
            let (width, height) = (w.width, w.height);
            let px = w.pixels(buffer);
            for y in 0..height {
                for x in 0..width {
                    let at = ((y * width + x) * 4) as usize;
                    px[at..at + 4].copy_from_slice(&pattern(seed as u32, x, y));
                }
            }
            let damage = [abi::Rect::new(0, 0, width as i32, height as i32)];
            match window::commit(id, buffer, &damage, None) {
                Ok(c) => {
                    w.held = None;
                    say(&format!("ok {} {} {} {} {}", c.commit_id, c.pixel_gen, buffer, base, full));
                }
                Err(e) => say(&format!("err {}", errno(e))),
            }
        }
        (Some("verify"), Some(id)) => {
            let (Some(seed), Some(w)) = (num(2), wins.get_mut(&id)) else {
                say("err 22");
                return true;
            };
            let (buffer, base, full) = match w.acquire(id) {
                Ok(held) => held,
                Err(e) => {
                    say(&format!("err {}", errno(e)));
                    return true;
                }
            };
            let (width, height) = (w.width, w.height);
            let px = w.pixels(buffer);
            for y in 0..height {
                for x in 0..width {
                    let at = ((y * width + x) * 4) as usize;
                    let want = pattern(seed as u32, x, y);
                    if px[at..at + 3] != want[..3] {
                        say(&format!(
                            "mismatch {x} {y} {:02x}{:02x}{:02x} {:02x}{:02x}{:02x}",
                            px[at], px[at + 1], px[at + 2], want[0], want[1], want[2]
                        ));
                        return true;
                    }
                }
            }
            say(&format!("ok {buffer} {base} {full}"));
        }
        (Some("destroy"), Some(id)) => match window::destroy(id) {
            Ok(_) => {
                wins.remove(&id);
                say("ok");
            }
            Err(e) => say(&format!("err {}", errno(e))),
        },
        (Some("quit"), _) => {
            say("ok");
            return false;
        }
        _ => say("err 22"),
    }
    true
}

fn main() {
    say("ready");
    let mut wins = BTreeMap::new();
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let mut buf = [0u8; 1024];
        match io::read(Fd::STDIN, &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => pending.extend_from_slice(&buf[..n]),
        }
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            if !run(&text, &mut wins) {
                return;
            }
        }
    }
}
