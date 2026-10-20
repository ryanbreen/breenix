//! kiosk_test: the breengel suite's test compositor (`docs/gui/breengel.md` section 24).
//!
//! It claims the compositor and prints `ready <epoch>`, or `claim-failed <errno>` and exits 1.
//! Then it reads one command per line on standard input and answers each with one line on
//! standard output: `ok ...` or `err <errno>`.
//!
//!   place <id> <x> <y> <z> <flags>   SET_GEOMETRY
//!   event <id> <hex> [<hex>...]      WRITE_EVENTS of these 320-byte events (640 hex digits
//!                                    each) in one call, each given the next input serial ->
//!                                    ok <queued> <first serial>
//!   burst <id> <count> <kind> <code> WRITE_EVENTS of <count> events of <kind> with keycodes
//!                                    <code>, <code> + 1, ... in one call -> ok <queued> <first serial>
//!   capture <id> <w> <h>             CAPTURE -> ok <commit_id> <width> <height> <FNV-1a of the pixels>
//!   badwrite <id>                    WRITE_EVENTS of one event from an unmapped page
//!   crosswrite <id>                  WRITE_EVENTS of two events, the second running into an
//!                                    unmapped page
//!   badcomposite                     COMPOSITE with an unmapped descriptor, then with a read-only
//!                                    one -> <first answer> <second answer>
//!   composite                        COMPOSITE -> ok <serial> <pixels>
//!   composite2                       two COMPOSITE calls at once, from two threads ->
//!                                    <first answer>, <second answer>
//!   record <serial>                  COMPOSITE_RECORD -> ok <serial> <pixels> <count> <flags>
//!                                    followed by <id>:<commit_id>:<pixel_gen>:<pixels> per entry
//!   cursor <shape> <visible> <x> <y> SET_CURSOR
//!   give <pid>                       GIVE_DISPLAY
//!   auto <0|1>                       compose after every commit it sees (on at start)
//!   release                          RELEASE_COMPOSITOR
//!   quit                             exit 0
//!
//! Input serials (section 19.3) start at 1 and increase by one for every event it writes.
//! With `auto` on it lists the windows every 2 ms and composes when any registered window's
//! commit id has changed since its last composite.

use std::collections::BTreeMap;

use libbreenix::io::{self, poll_events::POLLIN, PollFd};
use libbreenix::memory;
use libbreenix::types::Fd;
use libbreenix::window::{self, abi};
use abi::{op, Plain};

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

/// What `window(op, ptr, len)` returned, as a reply word pair: `ok` or `err <errno>`.
fn answer(ret: i64) -> String {
    if ret < 0 { format!("err {}", -ret) } else { String::from("ok") }
}

/// `pages` pages of fresh anonymous memory, the last `unmapped` of them unmapped again, and the
/// rest read-only when `readonly`.
fn pages(pages: usize, unmapped: usize, readonly: bool) -> Option<*mut u8> {
    let len = pages * 4096;
    let base = memory::mmap(core::ptr::null_mut(), len, memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).ok()?;
    let kept = (pages - unmapped) * 4096;
    if unmapped > 0 {
        // SAFETY: the tail lies inside the mapping just made.
        memory::munmap(unsafe { base.add(kept) }, unmapped * 4096).ok()?;
    }
    if readonly {
        memory::mprotect(base, kept, memory::PROT_READ).ok()?;
    }
    Some(base)
}

fn raw(op: u32, ptr: u64, len: usize) -> i64 {
    // SAFETY: the kernel checks every user pointer; a bad one is EFAULT.
    unsafe { window::raw_call(op, ptr as *mut u8, len) }
}

fn write_raw(id: u64, ptr: u64, count: u32) -> i64 {
    let mut w = abi::WriteEvents::ZERO;
    w.id = id;
    w.ptr = ptr;
    w.count = count;
    raw(op::WRITE_EVENTS, &mut w as *mut abi::WriteEvents as u64, core::mem::size_of::<abi::WriteEvents>())
}

struct Kiosk {
    auto: bool,
    /// The next input serial.
    serial: u64,
}

impl Kiosk {
    /// Give `events` the next serials and write them to window `id` in one call.
    fn send(&mut self, id: u64, events: &mut [abi::WindowEvent]) {
        let first = self.serial;
        for e in events.iter_mut() {
            e.serial = self.serial;
            self.serial += 1;
        }
        reply(window::write_events(id, events).map(|n| format!("{n} {first}")));
    }

    fn command(&mut self, line: &str) -> bool {
        let words: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| words.get(i).and_then(|w| w.parse::<i64>().ok());
        match words.first().copied() {
            Some("place") => match (num(1), num(2), num(3), num(4), num(5)) {
                (Some(id), Some(x), Some(y), Some(z), Some(flags)) => {
                    reply(window::set_geometry(id as u64, x as i32, y as i32, z as u32, flags as u32).map(|_| String::new()))
                }
                _ => say("err 22"),
            },
            Some("event") => {
                let events: Option<Vec<abi::WindowEvent>> = words.iter().skip(2).map(|t| hex_event(t)).collect();
                match (num(1), events) {
                    (Some(id), Some(mut events)) if !events.is_empty() => self.send(id as u64, &mut events),
                    _ => say("err 22"),
                }
            }
            Some("burst") => match (num(1), num(2), num(3), num(4)) {
                (Some(id), Some(count), Some(kind), Some(code)) if count > 0 => {
                    let mut events: Vec<abi::WindowEvent> = (0..count)
                        .map(|i| {
                            let mut e = abi::WindowEvent::ZERO;
                            e.kind = kind as u16;
                            e.keycode = (code + i) as u32;
                            e
                        })
                        .collect();
                    self.send(id as u64, &mut events);
                }
                _ => say("err 22"),
            },
            Some("capture") => match (num(1), num(2), num(3)) {
                (Some(id), Some(w), Some(h)) => {
                    let mut px = vec![0u8; (w * h * 4) as usize];
                    let mut c = abi::Capture::ZERO;
                    c.id = id as u64;
                    c.dst = px.as_mut_ptr() as u64;
                    c.dst_len = px.len() as u64;
                    reply(window::call(op::CAPTURE, &mut c).map(|_| {
                        let fnv = px.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
                        format!("{} {} {} {fnv:016x}", c.commit_id, c.width, c.height)
                    }));
                }
                _ => say("err 22"),
            },
            Some("badwrite") => match (num(1), pages(1, 1, false)) {
                (Some(id), Some(base)) => say(&answer(write_raw(id as u64, base as u64, 1))),
                _ => say("err 22"),
            },
            Some("crosswrite") => match (num(1), pages(2, 1, false)) {
                (Some(id), Some(base)) => {
                    let size = core::mem::size_of::<abi::WindowEvent>();
                    // The first event lies whole in the mapped page; the second runs past its end.
                    let at = base as usize + 4096 - size - size / 2;
                    let mut e = abi::WindowEvent::ZERO;
                    e.kind = abi::event_kind::KEY_PRESS;
                    e.keycode = 2;
                    // SAFETY: the first event's bytes lie inside the mapped page.
                    unsafe { core::ptr::write_unaligned(at as *mut abi::WindowEvent, e) };
                    say(&answer(write_raw(id as u64, at as u64, 2)));
                }
                _ => say("err 22"),
            },
            Some("badcomposite") => {
                let size = core::mem::size_of::<abi::Composite>();
                let first = pages(1, 1, false).map_or(-1, |p| raw(op::COMPOSITE, p as u64, size));
                let second = pages(1, 0, true).map_or(-1, |p| raw(op::COMPOSITE, p as u64, size));
                say(&format!("{} {}", answer(first), answer(second)));
            }
            Some("gpu-counters") => {
                let (ret,c)=libbreenix::gpu::counters(0);
                if ret==0 {say(&format!("ok {} {} {} {} {} {}",c.draws,c.instances,c.uploads_image,c.uploads_fixture,c.resources,c.resource_bytes));}
                else {say(&format!("err {}",-ret));}
            }
            Some("gpu-scanout") => {
                let mut observed=(0i64,0u32);
                let result=(||->Result<(),String>{
                    let path=*words.get(1).ok_or("missing output file")?;
                    let (ret,m)=libbreenix::gpu::modes();if ret!=0{return Err(format!("MODES {ret}"));}
                    let mode=m.modes[m.current as usize];
                    let mut pixels=vec![0xA5u8;mode.width as usize*mode.height as usize*4];
                    let mut record=[0xA5u8;libbreenix::gpu::abi::record::SIZE];
                    let (ret,arg)=libbreenix::gpu::readback_why(0,[0,0,mode.width,mode.height],&mut pixels,mode.width*4,&mut record);
                    observed=(ret,arg.reason);
                    let mut out=Vec::with_capacity(8+record.len()+pixels.len());
                    out.extend_from_slice(&mode.width.to_le_bytes());out.extend_from_slice(&mode.height.to_le_bytes());out.extend_from_slice(&record);out.extend_from_slice(&pixels);
                    std::fs::write(path,out).map_err(|e|e.to_string())
                })();
                match result{Ok(()) if observed.0==0=>say("ok"),Ok(())=>say(&format!("err {} reason {}",-observed.0,observed.1)),Err(e)=>say(&format!("gpu-display: {e}"))}
            }
            Some("composite") => reply(window::composite().map(|(serial, pixels)| format!("{serial} {pixels}"))),
            Some("composite2") => {
                let other = std::thread::spawn(|| window::composite());
                let mine = window::composite();
                let theirs = other.join().unwrap_or(Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EIO)));
                let word = |r: Result<(u64, u64), libbreenix::error::Error>| match r {
                    Ok((serial, pixels)) => format!("ok {serial} {pixels}"),
                    Err(e) => format!("err {}", errno(e)),
                };
                say(&format!("{}, {}", word(mine), word(theirs)));
            }
            Some("record") => match num(1) {
                Some(serial) => {
                    let mut buf = vec![0u8; 16 + 8 + 64 * 32];
                    let mut q = abi::RecordQuery::ZERO;
                    q.serial = serial as u64;
                    q.ptr = buf.as_mut_ptr() as u64;
                    q.len = buf.len() as u32;
                    reply(window::call(op::COMPOSITE_RECORD, &mut q).map(|_| {
                        // SAFETY: the kernel wrote a header and `count` entries into `buf`.
                        let h: abi::CompositeRecordHeader = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const _) };
                        let mut text = format!("{} {} {} {}", h.serial, h.pixels, h.count, h.flags);
                        let head = core::mem::size_of::<abi::CompositeRecordHeader>();
                        for i in 0..(h.count as usize).min(64) {
                            // SAFETY: as above.
                            let e: abi::CompositeEntry = unsafe {
                                core::ptr::read_unaligned(buf.as_ptr().add(head + i * core::mem::size_of::<abi::CompositeEntry>()) as *const _)
                            };
                            text.push_str(&format!(" {}:{}:{}:{}", e.id, e.commit_id, e.pixel_gen, e.pixels));
                        }
                        text
                    }));
                }
                None => say("err 22"),
            },
            Some("cursor") => match (num(1), num(2), num(3), num(4)) {
                (Some(shape), Some(visible), Some(x), Some(y)) => {
                    let mut c = abi::Cursor { shape: shape as u32, visible: visible as u32, x: x as i32, y: y as i32 };
                    reply(window::call(op::SET_CURSOR, &mut c).map(|_| String::new()));
                }
                _ => say("err 22"),
            },
            Some("give") => match num(1) {
                Some(pid) => {
                    let mut g = abi::Give { pid: pid as u32, reserved: 0 };
                    reply(window::call(op::GIVE_DISPLAY, &mut g).map(|_| String::new()));
                }
                None => say("err 22"),
            },
            Some("auto") => {
                self.auto = num(1) == Some(1);
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
    let mut kiosk = Kiosk { auto: true, serial: 1 };
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
                if !kiosk.command(&text) {
                    return;
                }
            }
        }
        if kiosk.auto {
            compose_new_commits(&mut seen);
        }
    }
}
