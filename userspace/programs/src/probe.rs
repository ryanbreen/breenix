//! Production boot probe: one bounded, honest result for each userspace milestone check.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::fs::{self, DirentIter, O_CREAT, O_DIRECTORY, O_RDONLY, O_TRUNC, O_WRONLY};
use libbreenix::graphics;
use libbreenix::io::{self, poll_events, PollFd};
use libbreenix::memory;
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::signal::{self, Sigaction, SIGKILL, SIGUSR1};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_DGRAM};
use libbreenix::termios::{self, Termios};
use libbreenix::time;
use libbreenix::types::Fd;
use libgfx::{color::Color, font, framebuf::FrameBuf, shapes};

const IDS: [&str; 16] = [
    "console", "getpid", "clock", "brk", "mmap", "fork-wait", "exec", "argv",
    "pipe", "signal", "poll", "file", "dir", "tty", "udp-loopback", "framebuffer",
];
const MEANINGS: [&str; 16] = [
    "Console output", "Process identity", "Monotonic clock", "Heap growth",
    "Memory mapping", "Fork and wait", "Program execution", "Program arguments",
    "Interprocess pipe", "Signal handler", "Readable pipe polling",
    "Ext2 file round trip", "Ext2 directory", "Terminal attributes",
    "UDP loopback", "Framebuffer drawing",
];
const DETAILS: [&str; 16] = [
    "stdout write succeeded", "PID is positive", "advanced after 10 ms sleep",
    "program break advanced", "mapped memory read back", "child exited 7",
    "exec child exited 0", "target validated argv", "child sent bytes to parent",
    "SIGUSR1 handler ran", "POLLIN reported", "write/read/unlink succeeded",
    "mkdir/list/rmdir succeeded", "isatty and tcgetattr succeeded",
    "datagram echoed on 127.0.0.1", "fb info and draw succeeded",
];
const GROUPS: [(&str, usize, usize); 7] = [
    ("FIRST USERSPACE", 0, 5), ("PROCESS LIFECYCLE", 5, 8),
    ("SIGNALS + IPC", 8, 11), ("USERSPACE FS", 11, 13),
    ("TTY + SHELL", 13, 14), ("NETWORKING", 14, 15),
    ("RUNTIMES", 15, 16),
];
static SIGNAL_SEEN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: i32) {
    SIGNAL_SEEN.store(true, Ordering::SeqCst);
}

fn check(index: usize) -> Result<(), &'static str> {
    match index {
        0 => {
            const HEADER: &[u8] = b"Breenix subsystem probe\n";
            if io::write(Fd::STDOUT, HEADER).ok() == Some(HEADER.len()) {
                Ok(())
            } else { Err("stdout write failed") }
        }
        1 => match process::getpid() {
            Ok(pid) if pid.raw() > 0 => Ok(()),
            _ => Err("getpid returned no PID"),
        },
        2 => {
            let before = time::now_monotonic().map_err(|_| "clock read failed")?;
            time::sleep_ms(10).map_err(|_| "nanosleep failed")?;
            let after = time::now_monotonic().map_err(|_| "second clock read failed")?;
            if after.as_nanos() > before.as_nanos() { Ok(()) }
            else { Err("monotonic clock did not advance") }
        }
        3 => {
            let old = memory::get_brk();
            let requested = old.checked_add(4096).ok_or("break overflow")?;
            if old == 0 || memory::brk(requested) < requested { return Err("brk did not grow"); }
            if memory::get_brk() >= requested { Ok(()) } else { Err("brk readback failed") }
        }
        4 => {
            let ptr = memory::mmap(std::ptr::null_mut(), 4096,
                memory::PROT_READ | memory::PROT_WRITE,
                memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0)
                .map_err(|_| "mmap failed")?;
            unsafe { ptr.write_volatile(0x5a); }
            let valid = unsafe { ptr.read_volatile() == 0x5a };
            memory::munmap(ptr, 4096).map_err(|_| "munmap failed")?;
            if valid { Ok(()) } else { Err("mapped bytes changed") }
        }
        5 => match process::fork().map_err(|_| "fork failed")? {
            ForkResult::Child => process::exit(7),
            ForkResult::Parent(pid) => {
                let mut status = 0;
                process::waitpid(pid.raw() as i32, &mut status, 0)
                    .map_err(|_| "waitpid failed")?;
                if process::wifexited(status) && process::wexitstatus(status) == 7 { Ok(()) }
                else { Err("child did not exit 7") }
            }
        },
        6 => exec_check(b"/bin/spawn_smoke_target\0", None),
        7 => {
            let argv = [b"exec_smoke_target\0".as_ptr(), b"smoke\0".as_ptr(), std::ptr::null()];
            exec_check(b"/bin/exec_smoke_target\0", Some(&argv))
        }
        8 => {
            let (reader, writer) = io::pipe().map_err(|_| "pipe failed")?;
            match process::fork().map_err(|_| "fork failed")? {
                ForkResult::Child => {
                    let _ = io::close(reader);
                    let ok = io::write(writer, b"pipe-ok").ok() == Some(7);
                    let _ = io::close(writer);
                    process::exit(if ok { 0 } else { 1 });
                }
                ForkResult::Parent(pid) => {
                    let _ = io::close(writer);
                    let mut buf = [0; 7];
                    let read = io::read(reader, &mut buf);
                    let _ = io::close(reader);
                    let mut status = 0;
                    process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|_| "waitpid failed")?;
                    if read.ok() == Some(7) && &buf == b"pipe-ok"
                        && process::wifexited(status) && process::wexitstatus(status) == 0 { Ok(()) }
                    else { Err("pipe data or child status wrong") }
                }
            }
        }
        9 => {
            SIGNAL_SEEN.store(false, Ordering::SeqCst);
            signal::sigaction(SIGUSR1, Some(&Sigaction::new(on_signal)), None)
                .map_err(|_| "sigaction failed")?;
            let pid = process::getpid().map_err(|_| "getpid failed")?;
            signal::kill(pid.raw() as i32, SIGUSR1).map_err(|_| "self signal failed")?;
            for _ in 0..1000 {
                if SIGNAL_SEEN.load(Ordering::SeqCst) { return Ok(()); }
                let _ = process::yield_now();
            }
            Err("signal handler did not run")
        }
        10 => {
            let (reader, writer) = io::pipe().map_err(|_| "pipe failed")?;
            let written = io::write(writer, b"p");
            let mut fds = [PollFd::new(reader, poll_events::POLLIN)];
            let ready = io::poll(&mut fds, 100);
            let _ = io::close(reader);
            let _ = io::close(writer);
            if written.ok() == Some(1) && ready.ok() == Some(1)
                && fds[0].revents & poll_events::POLLIN != 0 { Ok(()) }
            else { Err("poll did not report POLLIN") }
        }
        11 => {
            const PATH: &str = "/tmp/breenix-probe-file\0";
            let fd = fs::open_with_mode(PATH, O_WRONLY | O_CREAT | O_TRUNC, 0o600)
                .map_err(|_| "create failed")?;
            let written = fs::write(fd, b"breenix-probe");
            let _ = io::close(fd);
            if written.ok() != Some(13) { let _ = fs::unlink(PATH); return Err("write failed"); }
            let fd = fs::open(PATH, O_RDONLY).map_err(|_| "read open failed")?;
            let mut buf = [0; 13];
            let read = fs::read(fd, &mut buf);
            let _ = io::close(fd);
            let removed = fs::unlink(PATH);
            if read.ok() == Some(13) && &buf == b"breenix-probe" && removed.is_ok() { Ok(()) }
            else { Err("readback or unlink failed") }
        }
        12 => {
            const PATH: &str = "/tmp/breenix-probe-dir\0";
            const ENTRY: &str = "/tmp/breenix-probe-dir/item\0";
            fs::mkdir(PATH, 0o700).map_err(|_| "mkdir failed")?;
            let fd = fs::open_with_mode(ENTRY, O_WRONLY | O_CREAT | O_TRUNC, 0o600)
                .map_err(|_| "entry create failed")?;
            let _ = io::close(fd);
            let dir = fs::open(PATH, O_RDONLY | O_DIRECTORY).map_err(|_| "directory open failed")?;
            let mut buf = [0; 512];
            let found = match fs::getdents64(dir, &mut buf) {
                Ok(n) => DirentIter::new(&buf, n).any(|e| unsafe { e.name_str() == Some("item") }),
                Err(_) => false,
            };
            let _ = io::close(dir);
            let unlinked = fs::unlink(ENTRY);
            let removed = fs::rmdir(PATH);
            if found && unlinked.is_ok() && removed.is_ok() { Ok(()) }
            else { Err("list or rmdir failed") }
        }
        13 => {
            let mut attrs = Termios::default();
            if termios::isatty(Fd::STDOUT) && termios::tcgetattr(Fd::STDOUT, &mut attrs).is_ok() {
                Ok(())
            } else { Err("stdout is not a tty") }
        }
        14 => {
            let rx = socket::socket(AF_INET, SOCK_DGRAM, 0).map_err(|_| "socket failed")?;
            let addr = SockAddrIn::new([127, 0, 0, 1], 45454);
            socket::bind_inet(rx, &addr).map_err(|_| "bind failed")?;
            let tx = socket::socket(AF_INET, SOCK_DGRAM, 0).map_err(|_| "sender socket failed")?;
            let sent = socket::sendto(tx, b"udp-ok", &addr);
            let mut buf = [0; 6];
            let got = socket::recvfrom(rx, &mut buf, None);
            let _ = io::close(tx);
            let _ = io::close(rx);
            if sent.ok() == Some(6) && got.ok() == Some(6) && &buf == b"udp-ok" { Ok(()) }
            else { Err("loopback payload mismatch") }
        }
        15 => {
            let info = graphics::fbinfo().map_err(|_| "fbinfo failed")?;
            if info.width == 0 || info.height == 0 { return Err("empty framebuffer"); }
            graphics::fb_fill_rect(0, 0, 1, 1, graphics::rgb(36, 48, 70))
                .map_err(|_| "framebuffer draw failed")?;
            Ok(())
        }
        _ => Err("unknown check"),
    }
}

fn exec_check(path: &[u8], argv: Option<&[*const u8; 3]>) -> Result<(), &'static str> {
    match process::fork().map_err(|_| "fork failed")? {
        ForkResult::Child => {
            if let Some(args) = argv { let _ = process::execv(path, args.as_ptr()); }
            else { let _ = process::exec(path); }
            process::exit(127);
        }
        ForkResult::Parent(pid) => {
            let mut status = 0;
            process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|_| "waitpid failed")?;
            if process::wifexited(status) && process::wexitstatus(status) == 0 { Ok(()) }
            else { Err("exec target did not exit 0") }
        }
    }
}

fn monotonic_ms() -> Option<i128> { time::now_monotonic().ok().map(|ts| ts.as_nanos() / 1_000_000) }

fn supervised_check(index: usize) -> (Result<(), &'static str>, bool) {
    match process::fork() {
        Ok(ForkResult::Child) => process::exit(if check(index).is_ok() { 0 } else { 1 }),
        Ok(ForkResult::Parent(pid)) => {
            let started = monotonic_ms();
            let mut spins = 0u32;
            loop {
                let mut status = 0;
                match process::waitpid(pid.raw() as i32, &mut status, WNOHANG) {
                    Ok(done) if done.raw() == pid.raw() => {
                        return (if process::wifexited(status) && process::wexitstatus(status) == 0 {
                            Ok(())
                        } else { Err("verification failed") }, false);
                    }
                    Err(_) => return (Err("waitpid failed"), false),
                    _ => {}
                }
                spins += 1;
                if started.zip(monotonic_ms()).is_some_and(|(a, b)| b - a >= 2000)
                    || spins >= 200_000 {
                    let _ = signal::kill(pid.raw() as i32, SIGKILL);
                    let _ = process::waitpid(pid.raw() as i32, &mut status, WNOHANG);
                    return (Err("timeout"), false);
                }
                let _ = process::yield_now();
            }
        }
        Err(_) => {
            // Fork failure is a finding itself; inline checks are still useful where possible.
            (check(index), true)
        }
    }
}

fn dashboard(states: &[Option<bool>; 16], fb: &mut FrameBuf) -> Result<(), ()> {
    fb.clear(Color::rgb(12, 19, 31));
    font::draw_text(fb, b"BREENIX / BOOT PROBE", 16, 12, Color::WHITE, 2);
    font::draw_text(fb, b"Live subsystem status", 16, 35, Color::GRAY, 1);
    let mut y = 57i32;
    let gap = 6i32;
    let tile_w = ((fb.width as i32 - 32 - gap) / 2).max(80);
    for (title, first, end) in GROUPS {
        font::draw_text(fb, title.as_bytes(), 16, y as usize, Color::rgb(110, 168, 212), 1);
        y += 15;
        for (within, index) in (first..end).enumerate() {
            let x = 16 + (within % 2) as i32 * (tile_w + gap);
            let row_y = y + (within / 2) as i32 * 27;
            let fill = match states[index] {
                None => Color::rgb(57, 67, 80),
                Some(true) => Color::rgb(26, 112, 77),
                Some(false) => Color::rgb(145, 50, 58),
            };
            shapes::fill_rect(fb, x, row_y, tile_w, 23, fill);
            font::draw_text(fb, IDS[index].as_bytes(), (x + 8) as usize,
                (row_y + 7) as usize, Color::WHITE, 1);
        }
        y += ((end - first + 1) / 2) as i32 * 27 + 9;
    }
    graphics::fb_flush().map_err(|_| ())
}

fn main() {
    let mut states = [None; 16];
    let mut fb = graphics::fbinfo().ok().and_then(|info| {
        if info.left_pane_width() < 240 || info.height < 540 || !(3..=4).contains(&info.bytes_per_pixel) {
            return None;
        }
        let ptr = graphics::fb_mmap().ok()?;
        Some(unsafe { FrameBuf::from_raw(ptr, info.left_pane_width() as usize,
            info.height as usize, (info.left_pane_width() * info.bytes_per_pixel) as usize,
            info.bytes_per_pixel as usize, info.is_bgr()) })
    });
    if let Some(ref mut screen) = fb { let _ = dashboard(&states, screen); }

    println!("\x1b[1;36mBreenix boot probe — subsystem checklist\x1b[0m");
    let mut passed = 0;
    for index in 0..IDS.len() {
        let (result, inline) = supervised_check(index);
        states[index] = Some(result.is_ok());
        match result {
            Ok(()) => {
                passed += 1;
                let note = if inline { " (fork unavailable; ran inline)" } else { "" };
                println!("PROBE {} OK {}{}", IDS[index], DETAILS[index], note);
                println!("\x1b[32m✓\x1b[0m {} — {}: {}{}", IDS[index], MEANINGS[index], DETAILS[index], note);
            }
            Err(reason) => {
                let note = if inline { "fork unavailable; inline " } else { "" };
                println!("PROBE {} FAIL {}{}", IDS[index], note, reason);
                println!("\x1b[31m✗\x1b[0m {} — {}: {}{}", IDS[index], MEANINGS[index], note, reason);
            }
        }
        if let Some(ref mut screen) = fb { let _ = dashboard(&states, screen); }
    }
    println!("PROBE DONE passed={} failed={}", passed, IDS.len() - passed);
    println!("\x1b[1mProbe complete: {} passed, {} failed\x1b[0m", passed, IDS.len() - passed);
    loop {
        let mut status = 0;
        while process::waitpid(-1, &mut status, WNOHANG).is_ok_and(|pid| pid.raw() != 0) {}
        let _ = time::sleep_ms(100);
    }
}
