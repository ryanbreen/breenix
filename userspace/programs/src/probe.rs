//! Production boot probe: one bounded, honest result for each userspace milestone check.

mod probe_signal;

use libbreenix::fs::{self, DirentIter, O_CREAT, O_DIRECTORY, O_RDONLY, O_TRUNC, O_WRONLY};
use libbreenix::graphics;
use libbreenix::io::{self, poll_events, PollFd};
use libbreenix::memory;
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::signal::{self, SIGKILL};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_DGRAM};
use libbreenix::termios::{self, Termios};
use libbreenix::time;
use libbreenix::types::Fd;
use libgfx::{diagnostics::{self, Check, CheckState, Group, Panel, Verdict}, framebuf::FrameBuf};

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
    "ignored, blocked and caught signal waits passed", "POLLIN reported", "write/read/unlink succeeded",
    "mkdir/list/rmdir succeeded", "isatty and tcgetattr succeeded",
    "datagram echoed on 127.0.0.1", "fb info and draw succeeded",
];
const GROUPS: [(&str, usize, usize); 7] = [
    ("FIRST USERSPACE", 0, 5), ("PROCESS LIFECYCLE", 5, 8),
    ("SIGNALS + IPC", 8, 11), ("USERSPACE FS", 11, 13),
    ("TTY + SHELL", 13, 14), ("NETWORKING", 14, 15),
    ("RUNTIMES", 15, 16),
];
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
        9 => probe_signal::run(),
        10 => {
            let (reader, writer) = io::pipe().map_err(|_| "pipe failed")?;
            let written = io::write(writer, b"p");
            let mut fds = [PollFd::new(reader, poll_events::POLLIN)];
            // The byte is already in the pipe, so readiness needs no wait; a zero timeout
            // keeps this check non-blocking even when run inline.
            let ready = io::poll(&mut fds, 0);
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

/// Checks whose worst case is one bounded syscall. Only these run inline in PID 1 when
/// fork is unavailable; the rest wait on a sleep, another process, a socket or the disk,
/// and without a supervising child nothing could stop them hanging the probe.
const INLINE_SAFE: [bool; 16] = [
    true, true, false, true, true, false, false, false,
    false, false, true, false, false, true, false, true,
];
const LIMIT_NS: i128 = 2_000_000_000;
const REAP_LIMIT_NS: i128 = 500_000_000;
/// Consecutive polls with the monotonic clock unreadable or not moving before the
/// supervisor stops waiting on it. A working clock moves long before this is reached.
const STALLED_POLLS: u32 = 200_000;

fn monotonic_ns() -> Option<i128> { time::now_monotonic().ok().map(|ts| ts.as_nanos()) }

/// Measures a time limit on the monotonic clock. `poll` reports `Elapsed` once the limit
/// has passed, or `ClockStopped` once the clock has stopped (or never read) for the
/// deadline's stall limit of consecutive polls.
struct Deadline { start: Option<i128>, last: Option<i128>, stalled: u32, stall_limit: u32, limit: i128 }

enum Expiry { Running, Elapsed, ClockStopped }

impl Deadline {
    /// For loops that only yield between polls: `STALLED_POLLS` polls.
    fn new(limit: i128) -> Self { Self::with_stall_limit(limit, STALLED_POLLS) }

    /// For loops that wait between polls: size `stall_limit` so that many waits add
    /// up to no more than `limit`, so a stopped clock ends the wait within the limit.
    fn with_stall_limit(limit: i128, stall_limit: u32) -> Self {
        let now = monotonic_ns();
        Deadline { start: now, last: now, stalled: 0, stall_limit, limit }
    }

    fn poll(&mut self) -> Expiry {
        let now = monotonic_ns();
        if self.start.is_none() { self.start = now; }
        if self.start.zip(now).is_some_and(|(a, b)| b - a >= self.limit) { return Expiry::Elapsed; }
        if now.is_some() && now != self.last { self.last = now; self.stalled = 0; }
        else { self.stalled += 1; }
        if self.stalled >= self.stall_limit { Expiry::ClockStopped } else { Expiry::Running }
    }
}

/// Kill a check child that has run too long and reap it, saying truthfully if either failed.
fn stop_child(pid: i32, reason: &str) -> String {
    if signal::kill(pid, SIGKILL).is_err() {
        return format!("{reason}; SIGKILL failed, child may still run");
    }
    let mut deadline = Deadline::new(REAP_LIMIT_NS);
    loop {
        let mut status = 0;
        match process::waitpid(pid, &mut status, WNOHANG) {
            Ok(done) if done.raw() as i32 == pid => return reason.to_string(),
            Err(_) => return format!("{reason}; waitpid failed after SIGKILL"),
            _ => {}
        }
        if !matches!(deadline.poll(), Expiry::Running) {
            return format!("{reason}; child still alive after SIGKILL");
        }
        let _ = process::yield_now();
    }
}

/// The child's own failure reason, if it wrote one before exiting. Poll with a zero
/// timeout first: a grandchild may still hold the write end, so a bare read could block.
fn read_reason(reader: Fd) -> Option<String> {
    let mut fds = [PollFd::new(reader, poll_events::POLLIN)];
    if io::poll(&mut fds, 0).ok()? == 0 || fds[0].revents & poll_events::POLLIN == 0 {
        return None;
    }
    let mut buf = [0u8; 96];
    let n = io::read(reader, &mut buf).ok().filter(|&n| n > 0)?;
    std::str::from_utf8(&buf[..n]).ok().map(String::from)
}

fn child_verdict(status: i32, reader: Option<Fd>) -> Result<(), String> {
    if process::wifexited(status) && process::wexitstatus(status) == 0 { return Ok(()); }
    if let Some(reason) = reader.and_then(read_reason) { return Err(reason); }
    if process::wifexited(status) {
        Err(format!("check exited {} without a reason", process::wexitstatus(status)))
    } else if process::wifsignaled(status) {
        Err(format!("check killed by signal {}", process::wtermsig(status)))
    } else {
        Err("check ended abnormally".to_string())
    }
}

/// Run one check in a child and give it `LIMIT_NS`. Returns the result and whether it ran
/// inline because fork was unavailable (fork-failure reasons already say so).
fn supervised_check(index: usize) -> (Result<(), String>, bool) {
    // Close-on-exec so exec'd grandchildren never hold the reason channel open.
    let channel = io::pipe2(io::status_flags::O_CLOEXEC).ok();
    match process::fork() {
        Ok(ForkResult::Child) => {
            if let Some((reader, _)) = channel { let _ = io::close(reader); }
            let result = check(index);
            if let (Err(reason), Some((_, writer))) = (result, channel) {
                let _ = io::write(writer, reason.as_bytes());
            }
            process::exit(if result.is_ok() { 0 } else { 1 })
        }
        Ok(ForkResult::Parent(pid)) => {
            let pid = pid.raw() as i32;
            if let Some((_, writer)) = channel { let _ = io::close(writer); }
            let mut deadline = Deadline::new(LIMIT_NS);
            let result = loop {
                let mut status = 0;
                match process::waitpid(pid, &mut status, WNOHANG) {
                    Ok(done) if done.raw() as i32 == pid => {
                        break child_verdict(status, channel.map(|(reader, _)| reader));
                    }
                    Err(_) => break Err(stop_child(pid, "waitpid failed")),
                    _ => {}
                }
                match deadline.poll() {
                    Expiry::Running => {}
                    Expiry::Elapsed => break Err(stop_child(pid, "timeout")),
                    Expiry::ClockStopped => {
                        break Err(stop_child(pid, "timeout (clock stopped; 2 s limit unmeasurable)"));
                    }
                }
                let _ = process::yield_now();
            };
            if let Some((reader, _)) = channel { let _ = io::close(reader); }
            (result, false)
        }
        Err(_) => {
            if let Some((reader, writer)) = channel { let _ = io::close(reader); let _ = io::close(writer); }
            if INLINE_SAFE[index] {
                (check(index).map_err(|reason| format!("fork unavailable; inline {reason}")), true)
            } else {
                (Err("fork unavailable; not run inline because it could block".to_string()), true)
            }
        }
    }
}

/// Probe's screen mapping, and whether a program `probe --run` started has taken
/// the display from it.
struct Screen {
    fb: FrameBuf,
    lost: bool,
}

/// Take the display and map it: the whole screen, as the display owner, where the
/// kernel supports that.
fn open_screen() -> Option<Screen> {
    let info = graphics::fbinfo().ok()?;
    if info.width < 480 || info.height < 400
        || !(3..=4).contains(&info.bytes_per_pixel) { return None; }
    graphics::take_over_display().ok()?;
    let (ptr, width) = graphics::fb_mmap_pane().ok()?;
    if width < 240 { return None; }
    let fb = unsafe { FrameBuf::from_raw(ptr, width as usize, info.height as usize,
        (width * info.bytes_per_pixel) as usize, info.bytes_per_pixel as usize,
        info.is_bgr()) };
    Some(Screen { fb, lost: false })
}

/// Probe's screen, if it has one and still holds the display.
fn drawable(screen: &mut Option<Screen>) -> Option<&mut Screen> {
    screen.as_mut().filter(|screen| !screen.lost)
}

/// Put probe's screen on the display. A program run by `probe --run` that takes the
/// display itself (bwm, say) moves it away from probe, whose flushes are then
/// refused with EPERM: probe stops drawing and leaves the screen to that program
/// until it exits (`reclaim_screen`). Output still goes to serial.
fn flush_screen(screen: &mut Screen) {
    if let Err(libbreenix::error::Error::Os(libbreenix::Errno::EPERM)) = graphics::fb_flush() {
        screen.lost = true;
    }
}

/// The program has exited: take the display back, if it had taken it, to show the
/// verdict.
fn reclaim_screen(screen: &mut Option<Screen>) {
    if let Some(screen) = screen.as_mut() {
        if screen.lost && graphics::take_over_display().is_ok() {
            screen.lost = false;
        }
    }
}

fn draw_probe(screen: &mut Option<Screen>, states: &[Option<bool>; 16], details: &[String; 16],
    passed: usize, finished: bool) {
    let Some(screen) = drawable(screen) else { return; };
    let checks: Vec<_> = (0..IDS.len()).map(|index| Check {
        name: IDS[index],
        state: match states[index] {
            None => CheckState::Pending,
            Some(true) => CheckState::Ok(&details[index]),
            Some(false) => CheckState::Fail(&details[index]),
        },
    }).collect();
    let groups: Vec<_> = GROUPS.iter().map(|(title, first, end)| Group {
        title, checks: &checks[*first..*end],
    }).collect();
    let verdict = format!("{} passed / {} failed", passed, states.iter().filter(|s| **s == Some(false)).count());
    diagnostics::draw(&mut screen.fb, &Panel {
        title: "BREENIX / BOOT PROBE", subtitle: "Live subsystem status",
        groups: &groups, output: &[],
        verdict: if finished {
            if passed == IDS.len() { Verdict::Passed(&verdict) } else { Verdict::Failed(&verdict) }
        } else { Verdict::Running(&verdict) },
        scored: false,
        live: None,
    });
    flush_screen(screen);
}

fn reap_forever() -> ! {
    loop {
        let mut status = 0;
        while process::waitpid(-1, &mut status, WNOHANG).is_ok_and(|pid| pid.raw() != 0) {}
        let _ = time::sleep_ms(100);
    }
}

fn probe() {
    let mut states = [None; 16];
    let mut details: [String; 16] = std::array::from_fn(|_| String::new());
    let mut fb = open_screen();
    draw_probe(&mut fb, &states, &details, 0, false);

    println!("\x1b[1;36mBreenix boot probe — subsystem checklist\x1b[0m");
    let mut passed = 0;
    for index in 0..IDS.len() {
        let (result, inline) = supervised_check(index);
        states[index] = Some(result.is_ok());
        match result {
            Ok(()) => {
                passed += 1;
                let note = if inline { " (fork unavailable; ran inline)" } else { "" };
                details[index] = format!("{}{}", DETAILS[index], note);
                println!("PROBE {} OK {}{}", IDS[index], DETAILS[index], note);
                println!("\x1b[32m✓\x1b[0m {} — {}: {}{}", IDS[index], MEANINGS[index], DETAILS[index], note);
            }
            Err(reason) => {
                details[index] = reason.clone();
                println!("PROBE {} FAIL {}", IDS[index], reason);
                println!("\x1b[31m✗\x1b[0m {} — {}: {}", IDS[index], MEANINGS[index], reason);
            }
        }
        draw_probe(&mut fb, &states, &details, passed, false);
    }
    println!("PROBE DONE passed={} failed={}", passed, IDS.len() - passed);
    println!("\x1b[1mProbe complete: {} passed, {} failed\x1b[0m", passed, IDS.len() - passed);
    draw_probe(&mut fb, &states, &details, passed, true);
    reap_forever();
}

fn draw_run(screen: &mut Option<Screen>, path: &str, elapsed: i128,
    output: &[String], partial: &[u8], verdict: Verdict<'_>) {
    let Some(screen) = drawable(screen) else { return; };
    let subtitle = format!("{} seconds elapsed", elapsed.max(0) / 1_000_000_000);
    let checks = [Check { name: path, state: match &verdict {
        Verdict::Running(_) => CheckState::Running,
        Verdict::Passed(_) => CheckState::Ok("exit 0; no FAIL output"),
        Verdict::Failed(reason) => CheckState::Fail(reason),
    }}];
    let groups = [Group { title: "PROGRAM", checks: &checks }];
    let partial = String::from_utf8_lossy(partial);
    let mut lines: Vec<_> = output.iter().map(String::as_str).collect();
    if !partial.is_empty() { lines.push(&partial); }
    diagnostics::draw(&mut screen.fb, &Panel {
        title: "BREENIX / PROGRAM RUN", subtitle: &subtitle,
        groups: &groups, output: &lines, verdict, scored: false, live: None,
    });
    flush_screen(screen);
}

fn write_serial(mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        match io::write(Fd::STDOUT, bytes) {
            Ok(0) | Err(_) => return false,
            Ok(n) => bytes = &bytes[n..],
        }
    }
    true
}

fn add_output_byte(byte: u8, pending: &mut Vec<u8>, recent: &mut Vec<String>,
    fail_tail: &mut Vec<u8>, saw_fail: &mut bool, fail_line: &mut Option<String>) {
    if byte == b'\n' {
        let text = String::from_utf8_lossy(pending).trim_end_matches('\r').to_string();
        if *saw_fail && fail_line.is_none() {
            *fail_line = Some(if text.contains("FAIL") { text.clone() }
                else { format!("FAIL in long output line: {text}") });
        }
        recent.push(text);
        if recent.len() > 30 { recent.remove(0); }
        pending.clear();
        fail_tail.clear();
        return;
    }
    if pending.len() < 512 { pending.push(byte); }
    fail_tail.push(byte);
    if fail_tail.len() > 4 { fail_tail.remove(0); }
    if fail_tail.as_slice() == b"FAIL" { *saw_fail = true; }
}

/// `probe --run` time limit, and the poll wait of its supervising loop.
const RUN_LIMIT_NS: i128 = 60_000_000_000;
const RUN_POLL_MS: i32 = 100;
/// New output is drawn at most this often, so a chatty program never waits on the panel.
const RUN_DRAW_INTERVAL_NS: i128 = 250_000_000;
/// Limit for the child to reach exec, measured from fork.
const EXEC_LIMIT_NS: i128 = 10_000_000_000;

/// Polls of `RUN_POLL_MS` that add up to `limit`.
fn run_polls(limit: i128) -> u32 { (limit / (RUN_POLL_MS as i128 * 1_000_000)) as u32 }

/// Wait until the child has exec'd. Its close-on-exec status pipe reaches end of file with
/// no data once exec succeeds, and carries the reason when exec fails.
fn wait_for_exec(reader: Fd) -> Result<(), String> {
    let mut deadline = Deadline::with_stall_limit(EXEC_LIMIT_NS, run_polls(EXEC_LIMIT_NS));
    loop {
        let mut fds = [PollFd::new(reader, poll_events::POLLIN)];
        match io::poll(&mut fds, RUN_POLL_MS) {
            Ok(0) => {}
            Ok(_) => {
                let mut buf = [0u8; 96];
                return match io::read(reader, &mut buf) {
                    Ok(0) => Ok(()),
                    Ok(n) => Err(String::from_utf8_lossy(&buf[..n]).to_string()),
                    Err(_) => Err("exec status read failed".to_string()),
                };
            }
            Err(_) => return Err("exec status poll failed".to_string()),
        }
        match deadline.poll() {
            Expiry::Running => {}
            Expiry::Elapsed => return Err("program did not reach exec within 10 seconds".to_string()),
            Expiry::ClockStopped => {
                return Err("program did not reach exec (clock stopped; 10 s limit unmeasurable)".to_string());
            }
        }
    }
}

fn run_program(path: &str, args: &[String]) {
    let mut fb = open_screen();
    let mut recent = Vec::new();
    let start = monotonic_ns();
    draw_run(&mut fb, path, 0, &recent, &[], Verdict::Running("Program starting"));

    let mut failure = None;
    let mut status = None;
    let mut started = false;
    let mut timed_out = false;
    let mut clock_stopped = false;
    if !path.starts_with('/') || path.as_bytes().contains(&0)
        || args.iter().any(|arg| arg.as_bytes().contains(&0)) {
        failure = Some("invalid program path or argument".to_string());
    } else if start.is_none() {
        failure = Some("monotonic clock unavailable".to_string());
    } else if let (Ok((reader, writer)), Ok((exec_reader, exec_writer))) =
        (io::pipe(), io::pipe2(io::status_flags::O_CLOEXEC)) {
        match process::fork() {
            Ok(ForkResult::Child) => {
                let _ = io::close(reader);
                let _ = io::close(exec_reader);
                if io::dup2(writer, Fd::STDOUT).is_err()
                    || io::dup2(writer, Fd::STDERR).is_err() {
                    let _ = io::write(exec_writer, b"could not redirect output");
                    process::exit(127);
                }
                let _ = io::close(writer);
                let path_bytes = [path.as_bytes(), b"\0"].concat();
                let argv_bytes: Vec<Vec<u8>> = std::iter::once(path)
                    .chain(args.iter().map(String::as_str))
                    .map(|arg| [arg.as_bytes(), b"\0"].concat()).collect();
                let mut argv: Vec<*const u8> = argv_bytes.iter().map(|arg| arg.as_ptr()).collect();
                argv.push(std::ptr::null());
                let Err(error) = process::execv(&path_bytes, argv.as_ptr());
                let _ = io::write(exec_writer, format!("exec failed: {error:?}").as_bytes());
                process::exit(127);
            }
            Ok(ForkResult::Parent(pid)) => {
                let _ = io::close(writer);
                let _ = io::close(exec_writer);
                let pid = pid.raw() as i32;
                match wait_for_exec(exec_reader) {
                    Ok(()) => {
                        started = true;
                        println!("RUN {} START", path);
                    }
                    Err(reason) => failure = Some(reason),
                }
                let _ = io::close(exec_reader);
                let mut pending = Vec::new();
                let mut fail_tail = Vec::new();
                let mut saw_fail = false;
                let mut fail_line = None;
                let mut eof = false;
                let mut last_second = -1;
                let mut last_draw = 0;
                let mut output_dirty = false;
                let mut deadline = Deadline::with_stall_limit(RUN_LIMIT_NS, run_polls(RUN_LIMIT_NS));
                let mut kill_deadline = None;
                while started {
                    let elapsed = monotonic_ns().zip(start).map(|(now, then)| now - then).unwrap_or(0);
                    let second = elapsed / 1_000_000_000;
                    if second != last_second
                        || output_dirty && elapsed - last_draw >= RUN_DRAW_INTERVAL_NS {
                        draw_run(&mut fb, path, elapsed, &recent, &pending, Verdict::Running("Program running"));
                        last_second = second;
                        last_draw = elapsed;
                        output_dirty = false;
                    }
                    if !timed_out {
                        let expiry = deadline.poll();
                        if !matches!(expiry, Expiry::Running) {
                            timed_out = true;
                            clock_stopped = matches!(expiry, Expiry::ClockStopped);
                            kill_deadline = Some(Deadline::with_stall_limit(REAP_LIMIT_NS, run_polls(REAP_LIMIT_NS)));
                            if status.is_some() {
                                failure = Some(if clock_stopped {
                                    "output pipe remained open (clock stopped; 60 s limit unmeasurable)".to_string()
                                } else {
                                    "output pipe remained open after 60 seconds".to_string()
                                });
                                break;
                            } else if signal::kill(pid, SIGKILL).is_err() {
                                failure = Some("timeout; SIGKILL failed".to_string());
                            }
                        }
                    }
                    if status.is_none() {
                        let mut raw = 0;
                        match process::waitpid(pid, &mut raw, WNOHANG) {
                            Ok(done) if done.raw() as i32 == pid => status = Some(raw),
                            Err(_) => { failure = Some("waitpid failed".to_string()); break; }
                            _ => {}
                        }
                    }
                    if eof && status.is_some() { break; }
                    if kill_deadline.as_mut().is_some_and(|limit: &mut Deadline|
                        !matches!(limit.poll(), Expiry::Running)) { break; }
                    let mut fds = [PollFd::new(reader, poll_events::POLLIN)];
                    match io::poll(&mut fds, RUN_POLL_MS) {
                        Ok(0) => {}
                        Ok(_) if fds[0].revents & (poll_events::POLLIN | poll_events::POLLHUP) != 0 => {
                            let mut buf = [0u8; 1024];
                            match io::read(reader, &mut buf) {
                                Ok(0) => eof = true,
                                Ok(n) => {
                                    if !write_serial(&buf[..n]) { failure = Some("serial write failed".to_string()); }
                                    for &byte in &buf[..n] {
                                        add_output_byte(byte, &mut pending, &mut recent,
                                            &mut fail_tail, &mut saw_fail, &mut fail_line);
                                    }
                                    output_dirty = true;
                                }
                                Err(_) => { failure = Some("output read failed".to_string()); break; }
                            }
                        }
                        Ok(_) => { failure = Some("output pipe failed".to_string()); break; }
                        Err(_) => { failure = Some("output poll failed".to_string()); break; }
                    }
                }
                if !pending.is_empty() {
                    let text = String::from_utf8_lossy(&pending).to_string();
                    if saw_fail && fail_line.is_none() {
                        fail_line = Some(if text.contains("FAIL") { text.clone() }
                            else { format!("FAIL in long output line: {text}") });
                    }
                    recent.push(text);
                }
                let _ = io::close(reader);
                if status.is_none() {
                    if !timed_out && signal::kill(pid, SIGKILL).is_err() && failure.is_none() {
                        failure = Some("could not kill child after I/O failure".to_string());
                    }
                    let mut reap_deadline = Deadline::new(REAP_LIMIT_NS);
                    loop {
                        let mut raw = 0;
                        match process::waitpid(pid, &mut raw, WNOHANG) {
                            Ok(done) if done.raw() as i32 == pid => { status = Some(raw); break; }
                            Err(_) => break,
                            _ => {}
                        }
                        if !matches!(reap_deadline.poll(), Expiry::Running) { break; }
                        let _ = process::yield_now();
                    }
                    if status.is_none() && failure.is_none() {
                        failure = Some("child could not be reaped".to_string());
                    }
                }
                if timed_out && failure.is_none() {
                    failure = Some(if clock_stopped {
                        "timeout (clock stopped; 60 s limit unmeasurable)".to_string()
                    } else {
                        "timeout after 60 seconds".to_string()
                    });
                }
                if failure.is_none() { failure = fail_line.map(|line| format!("FAIL output: {line}")); }
            }
            Err(_) => {
                for fd in [reader, writer, exec_reader, exec_writer] { let _ = io::close(fd); }
                failure = Some("fork failed".to_string());
            }
        }
    } else {
        failure = Some("pipe failed".to_string());
    }

    // EXIT and SIGNAL describe the program, so they follow only a START.
    if let Some(raw) = status.filter(|_| started) {
        if process::wifexited(raw) {
            let code = process::wexitstatus(raw);
            println!("RUN {} EXIT {}", path, code);
            if code != 0 && failure.is_none() { failure = Some(format!("exit code {code}")); }
        } else if process::wifsignaled(raw) {
            let signal = process::wtermsig(raw);
            println!("RUN {} SIGNAL {}", path, signal);
            if failure.is_none() { failure = Some(format!("signal {signal}")); }
        } else if failure.is_none() { failure = Some("abnormal child status".to_string()); }
    }
    let elapsed = monotonic_ns().zip(start).map(|(now, then)| now - then).unwrap_or(0);
    reclaim_screen(&mut fb);
    if let Some(reason) = failure {
        println!("RUN {} DONE FAIL {}", path, reason);
        draw_run(&mut fb, path, elapsed, &recent, &[], Verdict::Failed(&reason));
    } else {
        println!("RUN {} DONE PASS exit 0; no FAIL output", path);
        draw_run(&mut fb, path, elapsed, &recent, &[], Verdict::Passed("PASS: exit 0; no FAIL output"));
    }
    reap_forever();
}

fn main() {
    let mut args = std::env::args();
    let _ = args.next();
    if args.next().as_deref() == Some("--run") {
        if let Some(path) = args.next() { run_program(&path, &args.collect::<Vec<_>>()); }
        else { run_program("", &[]); }
    } else { probe(); }
}
