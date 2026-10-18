//! The virtual terminals' cases (V4): `vt-devices`, `vt-switch` and
//! `scrollback`, measuring `docs/console/contract.md` sections 1, 3, 4 and 8.
//!
//! Cases switch terminals with `VT_ACTIVATE` and `VT_WAITACTIVE` on `/dev/tty0`,
//! read a terminal's cells through `/dev/vcsN` and `/dev/vcsaN`, read the
//! displayed pixels through `/proc/console/check`, and read the kernel's switch
//! lines from `/proc/kmsg`. A case that switches puts tty1 back in the
//! foreground when it ends (`BackToTty1`), after reaping the processes it
//! started (`Child`), so the screen cases and the runner's final panel find
//! the console as they expect. Keys come from the launcher (`HOSTKEY`, V6);
//! the `keys` cases (V5) are in `suite_console_keys.rs`.

use super::{identical, measured, require, screen_check, Verdict, O_NOCTTY};
use core::sync::atomic::{AtomicU32, Ordering};
use libbreenix::fs;
use libbreenix::io;
use libbreenix::signal;
use libbreenix::suite::CaseResult;
use libbreenix::syscall::{nr, raw};
use libbreenix::types::Fd;

#[path = "suite_console_keys.rs"]
mod keys;
pub use keys::*;

const VT_OPENQRY: u64 = 0x5600;
const VT_GETMODE: u64 = 0x5601;
const VT_SETMODE: u64 = 0x5602;
const VT_GETSTATE: u64 = 0x5603;
const VT_RELDISP: u64 = 0x5605;
const VT_ACTIVATE: u64 = 0x5606;
const VT_WAITACTIVE: u64 = 0x5607;
const KDSETMODE: u64 = 0x4B3A;
const KDGETMODE: u64 = 0x4B3B;
/// Breenix's request to move a terminal's scrollback view (contract 1.6).
const VT_SCROLLVIEW: u64 = 0x56B0;
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCFLSH: u64 = 0x540B;
const TIOCSCTTY: u64 = 0x540E;
const TIOCGPGRP: u64 = 0x540F;
const TIOCGWINSZ: u64 = 0x5413;
const TIOCSWINSZ: u64 = 0x5414;
const FIONREAD: u64 = 0x541B;
const TIOCGSID: u64 = 0x5429;

const EPERM: i64 = 1;
const EINTR: i64 = 4;
const ENXIO: i64 = 6;
const EBUSY: i64 = 16;
const EINVAL: i64 = 22;
const ENOTTY: i64 = 25;

const VT_AUTO: u8 = 0;
const VT_PROCESS: u8 = 1;
const VT_ACKACQ: u64 = 2;
const KD_TEXT: u64 = 0;
const KD_GRAPHICS: u64 = 1;
const WNOHANG: i32 = 1;

/// `struct termios` as TCGETS and TCSETS carry it (`linux/termbits.h`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Termios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    line: u8,
    cc: [u8; 19],
}
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const VTIME: usize = 5;
const VMIN: usize = 6;

fn sleep(ms: u64) {
    let _ = libbreenix::time::sleep_ms(ms);
}

fn now_ms() -> u64 {
    libbreenix::time::now_monotonic().map_or(0, |t| t.tv_sec as u64 * 1000 + t.tv_nsec as u64 / 1_000_000)
}

/// Poll `ready` at 10 ms intervals for up to `ms`.
fn within(ms: u64, mut ready: impl FnMut() -> bool) -> bool {
    let end = now_ms() + ms;
    loop {
        if ready() {
            return true;
        }
        if now_ms() >= end {
            return false;
        }
        sleep(10);
    }
}

/// A terminal opened without becoming the controlling terminal; closed on drop.
struct Term {
    fd: Fd,
    name: String,
}

impl Term {
    fn open(path: &str) -> Verdict<Term> {
        let fd = fs::open(path, fs::O_RDWR | O_NOCTTY).map_err(|e| format!("open {path}: {e}"))?;
        Ok(Term { fd, name: path.into() })
    }

    fn vt(n: usize) -> Verdict<Term> {
        Term::open(&format!("/dev/tty{n}"))
    }

    fn ioctl(&self, request: u64, arg: u64) -> i64 {
        // SAFETY: each request here takes a value or a pointer to a live local
        // of the size the published header gives it.
        unsafe { raw::syscall3(nr::IOCTL, self.fd.raw(), request, arg) as i64 }
    }

    fn ok(&self, what: &str, request: u64, arg: u64) -> Verdict<i64> {
        let ret = self.ioctl(request, arg);
        if ret < 0 { Err(format!("{what} on {}: errno {}", self.name, -ret)) } else { Ok(ret) }
    }

    fn fails(&self, what: &str, request: u64, arg: u64, errno: i64) -> Verdict {
        let ret = self.ioctl(request, arg);
        require(ret == -errno, &format!("{what} on {}: returned {ret}, expected errno {errno}", self.name))
    }

    fn write(&self, text: &[u8]) -> Verdict {
        let mut bytes = text;
        while !bytes.is_empty() {
            let n = io::write(self.fd, bytes).map_err(|e| format!("write {}: {e}", self.name))?;
            require(n != 0, "terminal write made no progress")?;
            bytes = &bytes[n..];
        }
        Ok(())
    }

    /// `VT_GETSTATE`: the foreground, `v_signal` and the open terminals.
    fn state(&self) -> Verdict<[u16; 3]> {
        let mut state = [0u16; 3];
        self.ok("VT_GETSTATE", VT_GETSTATE, state.as_mut_ptr() as u64)?;
        Ok(state)
    }

    fn termios(&self) -> Verdict<Termios> {
        let mut termios = Termios::default();
        self.ok("TCGETS", TCGETS, &mut termios as *mut Termios as u64)?;
        Ok(termios)
    }

    fn set_termios(&self, termios: &Termios) -> Verdict {
        self.ok("TCSETS", TCSETS, termios as *const Termios as u64).map(drop)
    }

    /// Raw input, one byte at a time, without echo.
    fn raw_mode(&self) -> Verdict<Termios> {
        let original = self.termios()?;
        let mut raw_mode = original;
        raw_mode.lflag &= !(ICANON | ECHO);
        raw_mode.cc[VMIN] = 1;
        raw_mode.cc[VTIME] = 0;
        self.set_termios(&raw_mode)?;
        self.ok("TCFLSH", TCFLSH, 0)?;
        Ok(original)
    }

    fn winsize(&self) -> Verdict<[u16; 4]> {
        let mut size = [0u16; 4];
        self.ok("TIOCGWINSZ", TIOCGWINSZ, size.as_mut_ptr() as u64)?;
        Ok(size)
    }

    fn available(&self) -> i32 {
        let mut n = 0i32;
        if self.ioctl(FIONREAD, &mut n as *mut i32 as u64) < 0 {
            return -1;
        }
        n
    }

    /// `VT_SCROLLVIEW`: move the view `lines` back (negative: forward); the
    /// lines it is then back from the live screen.
    fn scroll(&self, lines: i64) -> Verdict<i64> {
        self.ok("VT_SCROLLVIEW", VT_SCROLLVIEW, lines as u64)
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = io::close(self.fd);
    }
}

fn tty0() -> Verdict<Term> {
    Term::open("/dev/tty0")
}

fn activate(n: usize) -> Verdict {
    tty0()?.ok("VT_ACTIVATE", VT_ACTIVATE, n as u64).map(drop)
}

fn wait_active(n: usize) -> Verdict {
    tty0()?.ok("VT_WAITACTIVE", VT_WAITACTIVE, n as u64).map(drop)
}

fn switch_to(n: usize) -> Verdict {
    activate(n)?;
    wait_active(n)
}

fn foreground() -> Verdict<usize> {
    Ok(usize::from(tty0()?.state()?[0]))
}

/// Puts tty1 back in the foreground when the case ends, whatever it did, and
/// waits for that switch's acquisition to complete: `v_active` names tty1 as
/// soon as it starts, and a request made before it completes is held
/// (contract section 4.4). The case's children are reaped first (`Child` is
/// declared after this guard), so no `VT_PROCESS` owner holds the switch.
struct BackToTty1;

impl Drop for BackToTty1 {
    fn drop(&mut self) {
        let _ = switch_to(1);
    }
}

/// A process the case started: killed and reaped if still running at the end.
struct Child {
    pid: i32,
    /// Where it writes why it failed (non-blocking read end).
    reasons: Fd,
    reaped: bool,
}

impl Child {
    /// Run `body` in a child process; it exits 0 when it returns Ok, else 1
    /// after writing its reason.
    fn spawn(body: impl FnOnce() -> Verdict) -> Verdict<Child> {
        let (reasons, out) = io::pipe2(0o4000).map_err(|e| format!("pipe: {e}"))?;
        match libbreenix::process::fork().map_err(|e| format!("fork: {e}"))? {
            libbreenix::process::ForkResult::Child => {
                let code = match body() {
                    Ok(()) => 0,
                    Err(reason) => {
                        let _ = io::write(out, reason.as_bytes());
                        1
                    }
                };
                libbreenix::process::exit(code);
            }
            libbreenix::process::ForkResult::Parent(pid) => {
                let _ = io::close(out);
                Ok(Child { pid: pid.raw() as i32, reasons, reaped: false })
            }
        }
    }

    fn running(&mut self) -> bool {
        let mut status = 0;
        match libbreenix::process::waitpid(self.pid, &mut status, WNOHANG) {
            Ok(pid) if pid.raw() == 0 => true,
            _ => {
                self.reaped = true;
                false
            }
        }
    }

    /// Wait up to `ms` for the child to exit; Ok when it reported success.
    fn finish(&mut self, ms: u64) -> Verdict {
        let mut status = 0;
        let exited = within(ms, || match libbreenix::process::waitpid(self.pid, &mut status, WNOHANG) {
            Ok(pid) => pid.raw() != 0,
            Err(_) => true,
        });
        if !exited {
            return Err(format!("child {} still running after {ms} ms", self.pid));
        }
        self.reaped = true;
        if status == 0 {
            return Ok(());
        }
        let mut reason = [0u8; 512];
        let n = io::read(self.reasons, &mut reason).unwrap_or(0);
        if n == 0 {
            return Err(format!("child exited with status {status:#x}"));
        }
        Err(format!("child: {}", String::from_utf8_lossy(&reason[..n])))
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = signal::kill(self.pid, signal::SIGKILL);
            let mut status = 0;
            let _ = libbreenix::process::waitpid(self.pid, &mut status, 0);
        }
        let _ = io::close(self.reasons);
    }
}

/// A one-way signal between a case and its child: a byte down a pipe.
struct Gate {
    read: Fd,
    write: Fd,
}

impl Gate {
    fn new() -> Verdict<Gate> {
        let (read, write) = io::pipe2(0o4000).map_err(|e| format!("pipe: {e}"))?;
        Ok(Gate { read, write })
    }

    fn open(&self) {
        let _ = io::write(self.write, b"!");
    }

    /// Wait up to `ms` for the other side to open the gate.
    fn wait(&self, ms: u64, what: &str) -> Verdict {
        let mut byte = [0u8; 1];
        require(within(ms, || io::read(self.read, &mut byte).is_ok_and(|n| n == 1)), &format!("timed out waiting for {what}"))
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        let _ = io::close(self.read);
        let _ = io::close(self.write);
    }
}

/// Each terminal's screen text through `/dev/vcsN`: one string per row,
/// trailing blanks removed, bytes as Latin-1.
fn vcs_rows(n: usize) -> Verdict<Vec<String>> {
    let header = std::fs::read(format!("/dev/vcsa{n}")).map_err(|e| format!("read vcsa{n}: {e}"))?;
    require(header.len() >= 4, "vcsa shorter than its header")?;
    let cols = usize::from(header[1]);
    let text = std::fs::read(format!("/dev/vcs{n}")).map_err(|e| format!("read vcs{n}: {e}"))?;
    require(cols != 0 && text.len() % cols == 0, &format!("vcs{n} is {} bytes for {cols} columns", text.len()))?;
    Ok(text
        .chunks(cols)
        .map(|row| row.iter().map(|&b| char::from(b)).collect::<String>().trim_end().to_string())
        .collect())
}

fn vcs_has(n: usize, marker: &str) -> Verdict<bool> {
    Ok(vcs_rows(n)?.iter().any(|row| row.contains(marker)))
}

/// The rows `/proc/console/check` read back, after checking the screen
/// matched the shadow and the font, and the terminal it showed.
fn shown() -> Verdict<(usize, Vec<String>)> {
    let text = screen_check()?;
    identical(&text)?;
    require(text.lines().any(|l| l.starts_with("console oracle: ") && l.ends_with(" mismatches=0")), &text)?;
    let fg = text
        .lines()
        .find_map(|l| l.strip_prefix("console foreground: tty"))
        .and_then(|n| n.parse::<usize>().ok())
        .ok_or("check names no foreground terminal")?;
    let rows = text.lines().filter_map(|l| l.strip_prefix("console row: ")).map(String::from).collect();
    Ok((fg, rows))
}

/// Wait until the screen shows terminal `n` with a row containing `marker`.
fn shows(n: usize, marker: &str) -> Verdict<Vec<String>> {
    let mut last = String::new();
    for _ in 0..30 {
        match shown() {
            Ok((fg, rows)) if fg == n && rows.iter().any(|row| row.contains(marker)) => return Ok(rows),
            Ok((fg, rows)) => last = format!("tty{fg} rows {rows:?}"),
            Err(error) => last = error,
        }
        sleep(100);
    }
    Err(format!("tty{n} never showed {marker}: {last}"))
}

fn kmsg() -> Verdict<String> {
    std::fs::read_to_string("/proc/kmsg").map_err(|e| format!("kmsg: {e}"))
}

fn count(text: &str, pattern: &str) -> usize {
    text.matches(pattern).count()
}

fn mark(label: &str) -> String {
    format!("{label}-{}", libbreenix::process::getpid().map_or(0, |p| p.raw()))
}

static RELEASES: AtomicU32 = AtomicU32::new(0);
static ACQUIRES: AtomicU32 = AtomicU32::new(0);
static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_release(_: i32) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}
extern "C" fn on_acquire(_: i32) {
    ACQUIRES.fetch_add(1, Ordering::SeqCst);
}
extern "C" fn on_interrupt(_: i32) {
    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
}

fn handle(sig: i32, handler: extern "C" fn(i32)) -> Verdict {
    signal::sigaction(sig, Some(&signal::Sigaction::new(handler)), None).map_err(|e| format!("sigaction {sig}: {e}"))
}

/// `struct vt_mode` (`linux/vt.h`): mode, waitv, relsig, acqsig, frsig.
fn vt_mode(mode: u8, relsig: i16, acqsig: i16) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[0] = mode;
    bytes[2..4].copy_from_slice(&relsig.to_le_bytes());
    bytes[4..6].copy_from_slice(&acqsig.to_le_bytes());
    bytes
}

/// Make this process terminal `term`'s `VT_PROCESS` owner: SIGUSR1 asks it to
/// release, SIGUSR2 tells it it acquired.
fn own(term: &Term) -> Verdict {
    handle(signal::SIGUSR1, on_release)?;
    handle(signal::SIGUSR2, on_acquire)?;
    let mode = vt_mode(VT_PROCESS, signal::SIGUSR1 as i16, signal::SIGUSR2 as i16);
    term.ok("VT_SETMODE", VT_SETMODE, mode.as_ptr() as u64).map(drop)
}

fn signalled(counter: &AtomicU32, at_least: u32, ms: u64) -> bool {
    within(ms, || counter.load(Ordering::SeqCst) >= at_least)
}

// ---------------------------------------------------------------------------
// vt-devices
// ---------------------------------------------------------------------------

pub fn vt_devices_six_terminals() -> CaseResult {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    measured(|| {
        for (path, rdev) in (0..=6).map(|n| (format!("/dev/tty{n}"), (4u64 << 8) | n as u64)) {
            let meta = std::fs::metadata(&path).map_err(|e| format!("stat {path}: {e}"))?;
            require(meta.file_type().is_char_device(), &format!("{path} is not a character device"))?;
            require(meta.rdev() == rdev, &format!("{path} is device {:#x}, expected {rdev:#x}", meta.rdev()))?;
        }
        let terms = (1..=6).map(Term::vt).collect::<Verdict<Vec<_>>>()?;
        let state = terms[0].state()?;
        require(state[2] & 0b111_1110 == 0b111_1110, &format!("v_state {:#x} with each of tty1 to tty6 open", state[2]))?;
        for (n, term) in terms.iter().enumerate() {
            let label = mark(&format!("six-{}", n + 1));
            term.write(format!("\r\n{label}\r\n").as_bytes())?;
            require(vcs_has(n + 1, &label)?, &format!("tty{} did not take its own output", n + 1))?;
            for other in (1..=6).filter(|&o| o != n + 1) {
                require(!vcs_has(other, &label)?, &format!("tty{}'s output reached tty{other}", n + 1))?;
            }
        }
        Ok(())
    })
}

pub fn vt_devices_tty1_is_console() -> CaseResult {
    use std::os::unix::fs::MetadataExt;
    measured(|| {
        let console = Term::open("/dev/console")?;
        let tty1 = Term::vt(1)?;
        let rdev = std::fs::metadata("/dev/console").map_err(|e| format!("stat console: {e}"))?.rdev();
        require(rdev == (5 << 8) | 1, &format!("/dev/console is device {rdev:#x}"))?;
        let original = console.termios()?;
        let mut changed = original;
        changed.lflag ^= ECHO;
        console.set_termios(&changed)?;
        let seen = tty1.termios()?;
        console.set_termios(&original)?;
        require(seen.lflag == changed.lflag, "a termios change through /dev/console was not tty1's")?;
        let label = mark("console-is-tty1");
        tty1.write(format!("\r\n{label}\r\n").as_bytes())?;
        require(vcs_has(1, &label)?, "output to /dev/tty1 is not on tty1's screen")?;
        Ok(())
    })
}

pub fn vt_devices_tty0_is_foreground() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty0 = tty0()?;
        switch_to(3)?;
        let first = mark("tty0-on-3");
        tty0.write(format!("\r\n{first}\r\n").as_bytes())?;
        require(vcs_has(3, &first)? && !vcs_has(1, &first)?, "/dev/tty0 did not write tty3, the foreground")?;
        switch_to(2)?;
        let second = mark("tty0-on-2");
        tty0.write(format!("\r\n{second}\r\n").as_bytes())?;
        require(vcs_has(2, &second)? && !vcs_has(3, &second)?, "the same /dev/tty0 descriptor did not follow the foreground to tty2")?;
        let mut mode = -1i32;
        tty0.ok("KDGETMODE", KDGETMODE, &mut mode as *mut i32 as u64)?;
        require(mode == 0, "KDGETMODE on /dev/tty0 for text tty2")
    })
}

pub fn vt_devices_dev_tty_is_ctty() -> CaseResult {
    measured(|| {
        let label = mark("dev-tty");
        let probe = label.clone();
        let mut child = Child::spawn(move || {
            let sid = libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?.raw() as i32;
            let tty3 = Term::vt(3)?;
            tty3.ok("TIOCSCTTY", TIOCSCTTY, 0)?;
            let tty = Term::open("/dev/tty")?;
            tty.write(format!("\r\n{probe}\r\n").as_bytes())?;
            let mut session = 0i32;
            tty.ok("TIOCGSID", TIOCGSID, &mut session as *mut i32 as u64)?;
            require(session == sid, &format!("/dev/tty is in session {session}, the caller's is {sid}"))
        })?;
        child.finish(3000)?;
        require(vcs_has(3, &label)?, "output to /dev/tty did not reach tty3, the controlling terminal")
    })
}

pub fn vt_devices_dev_tty_enxio() -> CaseResult {
    measured(|| {
        Child::spawn(|| {
            libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
            match fs::open("/dev/tty", fs::O_RDWR) {
                Ok(fd) => {
                    let _ = io::close(fd);
                    Err("opening /dev/tty without a controlling terminal succeeded".into())
                }
                Err(e) => require(
                    matches!(e, libbreenix::error::Error::Os(libbreenix::errno::Errno::ENXIO)),
                    &format!("opening /dev/tty failed with {e}, not ENXIO"),
                ),
            }
        })?
        .finish(3000)
    })
}

pub fn vt_devices_termios_per_vt() -> CaseResult {
    measured(|| {
        let (tty2, tty3) = (Term::vt(2)?, Term::vt(3)?);
        let before = tty3.termios()?;
        let original = tty2.raw_mode()?;
        let other = tty3.termios()?;
        let mine = tty2.termios()?;
        tty2.set_termios(&original)?;
        require(mine.lflag & ICANON == 0, "tty2 did not keep its own change")?;
        require(other.lflag == before.lflag, "a change to tty2's termios changed tty3's")
    })
}

pub fn vt_devices_ctty_per_vt() -> CaseResult {
    measured(|| {
        Child::spawn(|| {
            let sid = libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?.raw() as i32;
            let tty2 = Term::vt(2)?;
            tty2.ok("TIOCSCTTY on tty2", TIOCSCTTY, 0)?;
            let mut inner = Child::spawn(|| {
                let sid = libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?.raw() as i32;
                let tty2 = Term::vt(2)?;
                tty2.fails("TIOCSCTTY of another session's tty2", TIOCSCTTY, 0, EPERM)?;
                let tty3 = Term::vt(3)?;
                tty3.ok("TIOCSCTTY on tty3", TIOCSCTTY, 0)?;
                let mut session = 0i32;
                tty3.ok("TIOCGSID", TIOCGSID, &mut session as *mut i32 as u64)?;
                require(session == sid, "tty3 is not the second session's terminal")?;
                tty2.fails("TIOCGSID of a terminal that is not the caller's", TIOCGSID, &mut session as *mut i32 as u64, ENOTTY)
            })?;
            inner.finish(3000)?;
            let mut session = 0i32;
            tty2.ok("TIOCGSID", TIOCGSID, &mut session as *mut i32 as u64)?;
            require(session == sid, "tty2 is not the first session's terminal")
        })?
        .finish(5000)
    })
}

/// The cell size and grid `/proc/console/check` reports.
fn display_grid() -> Verdict<(usize, usize, usize, usize)> {
    let text = screen_check()?;
    let line = text.lines().find_map(|l| l.strip_prefix("console display: ")).ok_or("missing display")?;
    let (_, rest) = line.split_once(" (").ok_or("missing cell size")?;
    let (cell, grid) = rest.split_once(" pixel cells), grid ").ok_or("missing grid size")?;
    let (cw, ch) = super::pair(cell)?;
    let (cols, rows) = super::pair(grid)?;
    Ok((cols, rows, cw, ch))
}

pub fn vt_devices_winsize_per_vt() -> CaseResult {
    measured(|| {
        let (cols, rows, cw, ch) = display_grid()?;
        for n in 1..=6 {
            let term = Term::vt(n)?;
            let size = term.winsize()?;
            let wanted = [rows as u16, cols as u16, (cols * cw) as u16, (rows * ch) as u16];
            require(size == wanted, &format!("tty{n} reports {size:?}, the size rule gives {wanted:?}"))?;
            term.ok("TIOCSWINSZ with its own size", TIOCSWINSZ, size.as_ptr() as u64)?;
            let other = [size[0] - 1, size[1], size[2], size[3]];
            term.fails("TIOCSWINSZ with another size", TIOCSWINSZ, other.as_ptr() as u64, EINVAL)?;
        }
        Ok(())
    })
}

pub fn vt_devices_vcs_text() -> CaseResult {
    measured(|| {
        let tty4 = Term::vt(4)?;
        let label = mark("vcs");
        // é is Latin-1 (0xE9); the box-drawing line U+2500 is outside it.
        tty4.write(format!("\x1b[H\x1b[2J{label} \u{e9} \u{2500}").as_bytes())?;
        let size = tty4.winsize()?;
        let (rows, cols) = (usize::from(size[0]), usize::from(size[1]));
        let text = std::fs::read("/dev/vcs4").map_err(|e| format!("read vcs4: {e}"))?;
        require(text.len() == rows * cols, &format!("vcs4 is {} bytes for a {cols}x{rows} grid", text.len()))?;
        let mut expected = label.into_bytes();
        expected.extend_from_slice(b" \xe9 ?");
        require(text.starts_with(&expected), &format!("vcs4 row 0 is {:?}", &text[..expected.len().min(text.len())]))?;
        require(text[expected.len()..cols].iter().all(|&b| b == b' '), "blank cells are not spaces")?;
        let fd = fs::open("/dev/vcs4", fs::O_RDWR).map_err(|e| format!("open vcs4: {e}"))?;
        let end = fs::lseek(fd, 0, fs::SEEK_END).ok();
        let mut byte = [0u8; 1];
        let at_end = io::read(fd, &mut byte);
        let wrote = io::write(fd, b"x");
        let _ = io::close(fd);
        require(end == Some((rows * cols) as u64), &format!("SEEK_END is {end:?}"))?;
        require(at_end.ok() == Some(0), "a read at the end did not return 0")?;
        require(wrote.is_err(), "a write to vcs4 succeeded")
    })
}

pub fn vt_devices_vcsa_attributes() -> CaseResult {
    measured(|| {
        let tty5 = Term::vt(5)?;
        tty5.write(b"\x1b[H\x1b[2J\x1b[31;44mA\x1b[0m\x1b[7mB\x1b[0m\x1b[8mC\x1b[0m")?;
        let size = tty5.winsize()?;
        let bytes = std::fs::read("/dev/vcsa5").map_err(|e| format!("read vcsa5: {e}"))?;
        let (rows, cols) = (usize::from(size[0]), usize::from(size[1]));
        require(bytes.len() == 4 + 2 * rows * cols, &format!("vcsa5 is {} bytes", bytes.len()))?;
        require(bytes[..4] == [rows as u8, cols as u8, 3, 0], &format!("header {:?}", &bytes[..4]))?;
        let cells: Vec<(u8, u8)> = bytes[4..14].chunks(2).map(|c| (c[0], c[1])).collect();
        // Red on blue; reverse of the default pair (7 on 0); invisible (0 on 0); a blank.
        let wanted = [(b'A', 0x41), (b'B', 0x70), (b'C', 0x00), (b' ', 0x07), (b' ', 0x07)];
        require(cells == wanted, &format!("cells {cells:?}, expected {wanted:?}"))
    })
}

pub fn vt_devices_background_output_retained() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let label = mark("retained");
        Term::vt(3)?.write(format!("\r\n{label}\r\n").as_bytes())?;
        require(foreground()? == 1, "writing to tty3 changed the foreground")?;
        require(vcs_has(3, &label)?, "tty3's grid lacks the output written while it was in the background")?;
        switch_to(3)?;
        shows(3, &label).map(drop)
    })
}

pub fn vt_devices_background_process_runs() -> CaseResult {
    measured(|| {
        let label = mark("bg");
        let child_label = label.clone();
        let mut child = Child::spawn(move || {
            let tty4 = Term::vt(4)?;
            for k in 0..50 {
                tty4.write(format!("{child_label}-{k:02}\r\n").as_bytes())?;
                sleep(5);
            }
            Ok(())
        })?;
        child.finish(5000)?;
        require(foreground()? != 4, "tty4 came to the foreground")?;
        require(vcs_has(4, &format!("{label}-49"))?, "the process on tty4 did not finish its output while tty4 was in the background")
    })
}

pub fn vt_devices_background_read_waits() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (tty2, tty3) = (Term::vt(2)?, Term::vt(3)?);
        let (saved2, saved3) = (tty2.raw_mode()?, tty3.raw_mode()?);
        let restore = || {
            let _ = tty2.set_termios(&saved2);
            let _ = tty3.set_termios(&saved3);
            let _ = tty2.ok("TCFLSH", TCFLSH, 0);
            let _ = tty3.ok("TCFLSH", TCFLSH, 0);
        };
        let result = (|| {
            switch_to(2)?;
            let mut child = Child::spawn(|| {
                let tty3 = Term::vt(3)?;
                let mut byte = [0u8; 1];
                let n = io::read(tty3.fd, &mut byte).map_err(|e| format!("read tty3: {e}"))?;
                require(n == 1 && byte[0] == b'y', &format!("tty3's reader got {:?}", &byte[..n]))
            })?;
            sleep(200);
            require(child.running(), "the read on tty3 returned before any input")?;
            libbreenix::suite::hostkeys("x")?;
            require(within(3000, || tty2.available() >= 1), "host key x did not reach tty2, the foreground")?;
            require(child.running(), "the key for tty2 ended the read on background tty3")?;
            switch_to(3)?;
            require(child.running(), "switching to tty3 ended its read without input")?;
            libbreenix::suite::hostkeys("y")?;
            child.finish(4000)
        })();
        restore();
        result
    })
}

pub fn vt_devices_switch_sends_no_signal() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty1 = Term::vt(1)?;
        let mut group = 0i32;
        tty1.ok("TIOCGPGRP", TIOCGPGRP, &mut group as *mut i32 as u64)?;
        let (ready, done) = (Gate::new()?, Gate::new()?);
        let mut child = Child::spawn(|| {
            libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
            let tty3 = Term::vt(3)?;
            tty3.ok("TIOCSCTTY", TIOCSCTTY, 0)?;
            let mut before = 0i32;
            tty3.ok("TIOCGPGRP", TIOCGPGRP, &mut before as *mut i32 as u64)?;
            for sig in [1, 18, 21, 22, 28] {
                handle(sig, on_interrupt)?;
            }
            ready.open();
            done.wait(8000, "the switches")?;
            let mut after = 0i32;
            tty3.ok("TIOCGPGRP", TIOCGPGRP, &mut after as *mut i32 as u64)?;
            require(INTERRUPTS.load(Ordering::SeqCst) == 0, "a switch signalled the process on tty3")?;
            require(before == after, "a switch changed tty3's foreground process group")
        })?;
        ready.wait(3000, "the process on tty3")?;
        for n in [3, 1, 3, 1] {
            switch_to(n)?;
        }
        done.open();
        child.finish(3000)?;
        let mut after = 0i32;
        tty1.ok("TIOCGPGRP", TIOCGPGRP, &mut after as *mut i32 as u64)?;
        require(after == group, "a switch changed tty1's foreground process group")
    })
}

// ---------------------------------------------------------------------------
// vt-switch
// ---------------------------------------------------------------------------

pub fn vt_switch_activate() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty0 = tty0()?;
        require(tty0.ok("VT_ACTIVATE", VT_ACTIVATE, 3)? == 0, "VT_ACTIVATE did not return 0")?;
        wait_active(3)?;
        require(tty0.state()?[0] == 3, "VT_GETSTATE does not name tty3")?;
        require(shown()?.0 == 3, "the screen does not show tty3")?;
        require(tty0.ok("VT_ACTIVATE of the foreground", VT_ACTIVATE, 3)? == 0, "VT_ACTIVATE of the foreground")
    })
}

pub fn vt_switch_activate_enxio() -> CaseResult {
    measured(|| {
        let tty0 = tty0()?;
        for n in [0, 7, 64] {
            tty0.fails("VT_ACTIVATE", VT_ACTIVATE, n, ENXIO)?;
            tty0.fails("VT_WAITACTIVE", VT_WAITACTIVE, n, ENXIO)?;
        }
        require(foreground()? == 1, "a refused VT_ACTIVATE switched")
    })
}

pub fn vt_switch_waitactive() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        wait_active(1)?;
        let mut child = Child::spawn(|| wait_active(4))?;
        sleep(200);
        require(child.running(), "VT_WAITACTIVE for tty4 returned while tty1 was in the foreground")?;
        activate(4)?;
        child.finish(3000)?;
        require(foreground()? == 4, "the waiter returned but tty4 is not in the foreground")
    })
}

pub fn vt_switch_waitactive_after_draw() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let label = mark("drawn");
        Term::vt(5)?.write(format!("\x1b[H\x1b[2J{label}\r\n").as_bytes())?;
        let line = "[console] foreground tty5 (ioctl)";
        let before = count(&kmsg()?, line);
        switch_to(5)?;
        // Logged once the frame is on the display, before waiters are woken.
        require(count(&kmsg()?, line) == before + 1, "VT_WAITACTIVE returned before the switch's frame completed")?;
        let (fg, rows) = shown()?;
        require(fg == 5 && rows.iter().any(|row| row.contains(&label)), &format!("tty{fg} rows {rows:?}"))
    })
}

pub fn vt_switch_waitactive_eintr() -> CaseResult {
    measured(|| {
        let mut child = Child::spawn(|| {
            handle(signal::SIGUSR1, on_interrupt)?;
            let ret = tty0()?.ioctl(VT_WAITACTIVE, 6);
            require(INTERRUPTS.load(Ordering::SeqCst) == 1, "the handler did not run")?;
            require(ret == -EINTR, &format!("VT_WAITACTIVE returned {ret}, expected EINTR"))
        })?;
        sleep(200);
        require(child.running(), "VT_WAITACTIVE for tty6 returned before the signal")?;
        signal::kill(child.pid, signal::SIGUSR1).map_err(|e| format!("kill: {e}"))?;
        child.finish(3000)?;
        require(foreground()? == 1, "the interrupted wait switched terminals")
    })
}

pub fn vt_switch_getstate() -> CaseResult {
    measured(|| {
        let tty5 = Term::vt(5)?;
        let state = tty5.state()?;
        require(usize::from(state[0]) == foreground()? && state[0] == 1, &format!("v_active {}", state[0]))?;
        require(state[1] == 0, "v_signal is not 0")?;
        require(state[2] & 1 == 0, "bit 0 of v_state is set")?;
        require(state[2] & (1 << 5) != 0 && state[2] & (1 << 1) != 0, &format!("v_state {:#x} with tty1 and tty5 open", state[2]))
    })
}

pub fn vt_switch_openqry() -> CaseResult {
    measured(|| {
        let tty0 = tty0()?;
        let query = || -> Verdict<i32> {
            let mut free = 0i32;
            tty0.ok("VT_OPENQRY", VT_OPENQRY, &mut free as *mut i32 as u64)?;
            Ok(free)
        };
        let first = query()?;
        require((2..=6).contains(&first), &format!("VT_OPENQRY answered {first}"))?;
        let _also_tty0 = Term::open("/dev/tty0")?;
        let _screen = std::fs::File::open(format!("/dev/vcs{first}")).map_err(|e| format!("open vcs: {e}"))?;
        require(query()? == first, "a descriptor for /dev/tty0 or a screen counted as open")?;
        let held = Term::vt(first as usize)?;
        let next = query()?;
        require(next == -1 || next > first, &format!("with tty{first} open VT_OPENQRY answered {next}"))?;
        drop(held);
        require(query()? == first, "closing the terminal did not free it")
    })
}

pub fn vt_switch_kdsetmode_graphics() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        switch_to(3)?;
        let tty3 = Term::vt(3)?;
        let restore = |tty3: &Term| tty3.ioctl(KDSETMODE, KD_TEXT);
        let result = (|| {
            tty3.ok("KDSETMODE(KD_GRAPHICS)", KDSETMODE, KD_GRAPHICS)?;
            let mut mode = -1i32;
            tty3.ok("KDGETMODE", KDGETMODE, &mut mode as *mut i32 as u64)?;
            require(mode == 1, "KDGETMODE after KD_GRAPHICS")?;
            require(screen_check()?.contains("console check: a process has the display"), "the kernel kept drawing tty3")?;
            let label = mark("kd-text");
            tty3.write(format!("\r\n{label}\r\n").as_bytes())?;
            require(vcs_has(3, &label)?, "output in KD_GRAPHICS was not kept in tty3's grid")?;
            tty3.ok("KDSETMODE(KD_TEXT)", KDSETMODE, KD_TEXT)?;
            shows(3, &label).map(drop)?;
            tty3.fails("KDSETMODE(2)", KDSETMODE, 2, EINVAL)
        })();
        restore(&tty3);
        result
    })
}

pub fn vt_switch_kd_owner_exit() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        switch_to(4)?;
        let label = mark("owner-exit");
        Term::vt(4)?.write(format!("\r\n{label}\r\n").as_bytes())?;
        let gone = "[console] tty4 owner gone: mode reset";
        let before = count(&kmsg()?, gone);
        let ready = Gate::new()?;
        let mut child = Child::spawn(|| {
            Term::vt(4)?.ok("KDSETMODE(KD_GRAPHICS)", KDSETMODE, KD_GRAPHICS)?;
            ready.open();
            sleep(300);
            Ok(())
        })?;
        ready.wait(3000, "the graphics owner")?;
        require(screen_check()?.contains("console check: a process has the display"), "KD_GRAPHICS did not stop the console")?;
        child.finish(3000)?;
        let mut mode = -1i32;
        Term::vt(4)?.ok("KDGETMODE", KDGETMODE, &mut mode as *mut i32 as u64)?;
        require(mode == 0, "tty4 stayed in KD_GRAPHICS after its owner exited")?;
        shows(4, &label)?;
        require(count(&kmsg()?, gone) == before + 1, "no owner-gone line")
    })
}

pub fn vt_switch_graphics_vt_refuses_switch() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        Child::spawn(|| {
            libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
            libbreenix::graphics::take_over_display().map_err(|e| format!("take display: {e}"))?;
            let line = "[console] switch to tty2 refused: tty1 display owned (ioctl)";
            let before = count(&kmsg()?, line);
            let tty0 = tty0()?;
            let refused = tty0.ioctl(VT_ACTIVATE, 2);
            let state = tty0.state()?;
            let mut mode = -1i32;
            Term::vt(1)?.ok("KDGETMODE", KDGETMODE, &mut mode as *mut i32 as u64)?;
            let logged = count(&kmsg()?, line) == before + 1;
            libbreenix::graphics::give_back_display().map_err(|e| format!("give back: {e}"))?;
            require(refused == -EBUSY, &format!("VT_ACTIVATE away from the display owner returned {refused}"))?;
            require(state[0] == 1, "the refused switch changed the foreground")?;
            require(mode == 1, "the display owner's terminal is not in KD_GRAPHICS")?;
            require(logged, "the refusal was not logged")?;
            switch_to(2)
        })?
        .finish(5000)
    })
}

/// Start a `VT_PROCESS` owner of tty3, with tty3 in the foreground. `body`
/// runs in the owner after it took the mode; `ready` opens once it has.
fn owner_on_tty3(ready: &Gate, body: impl FnOnce(&Term) -> Verdict) -> Verdict<Child> {
    switch_to(3)?;
    let child = Child::spawn(|| {
        let tty3 = Term::vt(3)?;
        own(&tty3)?;
        ready.open();
        body(&tty3)
    })?;
    ready.wait(3000, "the VT_PROCESS owner")?;
    Ok(child)
}

pub fn vt_switch_vt_process_release() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (ready, asked) = (Gate::new()?, Gate::new()?);
        let mut owner = owner_on_tty3(&ready, |tty3| {
            require(signalled(&RELEASES, 1, 3000), "no release signal")?;
            require(tty3.state()?[0] == 3, "the foreground moved before the owner answered")?;
            asked.open();
            sleep(200);
            tty3.ok("VT_RELDISP(1)", VT_RELDISP, 1).map(drop)
        })?;
        activate(2)?;
        asked.wait(3000, "the owner's release signal")?;
        require(foreground()? == 3, "the switch did not wait for VT_RELDISP")?;
        wait_active(2)?;
        owner.finish(3000)
    })
}

pub fn vt_switch_vt_process_acquire() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        switch_to(2)?;
        let (ready, acked) = (Gate::new()?, Gate::new()?);
        let mut owner = Child::spawn(|| {
            let tty3 = Term::vt(3)?;
            own(&tty3)?;
            ready.open();
            require(signalled(&ACQUIRES, 1, 3000), "no acquire signal")?;
            sleep(300);
            acked.open();
            tty3.ok("VT_RELDISP(VT_ACKACQ)", VT_RELDISP, VT_ACKACQ)?;
            tty3.fails("a second VT_ACKACQ", VT_RELDISP, VT_ACKACQ, EINVAL)?;
            // Done: back to automatic switching before the case leaves tty3.
            tty3.ok("VT_SETMODE(VT_AUTO)", VT_SETMODE, vt_mode(VT_AUTO, 0, 0).as_ptr() as u64).map(drop)
        })?;
        ready.wait(3000, "the VT_PROCESS owner")?;
        activate(3)?;
        wait_active(3)?;
        acked.wait(0, "the acknowledgement").map_err(|_| "VT_WAITACTIVE returned before VT_ACKACQ".to_string())?;
        owner.finish(3000)
    })
}

pub fn vt_switch_vt_process_refuse() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (ready, answered) = (Gate::new()?, Gate::new()?);
        let line = "[console] switch to tty2 refused by tty3 owner";
        let before = count(&kmsg()?, line);
        let mut owner = owner_on_tty3(&ready, |tty3| {
            require(signalled(&RELEASES, 1, 3000), "no release signal")?;
            tty3.ok("VT_RELDISP(0)", VT_RELDISP, 0)?;
            answered.open();
            sleep(300);
            Ok(())
        })?;
        activate(2)?;
        answered.wait(3000, "the refusal")?;
        sleep(100);
        require(foreground()? == 3, "the refused switch moved the foreground")?;
        require(count(&kmsg()?, line) == before + 1, "the refusal was not logged")?;
        owner.finish(3000)
    })
}

pub fn vt_switch_vt_process_release_timeout() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (ready, late) = (Gate::new()?, Gate::new()?);
        let line = "[console] switch to tty2 cancelled: tty3 owner did not release";
        let before = count(&kmsg()?, line);
        let mut owner = owner_on_tty3(&ready, |tty3| {
            require(signalled(&RELEASES, 1, 3000), "no release signal")?;
            late.wait(6000, "the deadline")?;
            tty3.fails("VT_RELDISP(1) after the deadline", VT_RELDISP, 1, EINVAL)
        })?;
        let asked = now_ms();
        activate(2)?;
        sleep(2500);
        require(foreground()? == 3, "the switch went ahead before the deadline")?;
        require(within(2000, || count(&kmsg().unwrap_or_default(), line) == before + 1), "no cancellation line")?;
        require(now_ms() - asked >= 3000, "cancelled before the 3 s deadline")?;
        require(foreground()? == 3, "the owner lost tty3")?;
        late.open();
        owner.finish(3000)
    })
}

pub fn vt_switch_vt_process_owner_exit() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let ready = Gate::new()?;
        let mut owner = owner_on_tty3(&ready, |_| {
            require(signalled(&RELEASES, 1, 3000), "no release signal")?;
            // Exit without answering.
            Ok(())
        })?;
        activate(2)?;
        owner.finish(3000)?;
        wait_active(2)?;
        require(foreground()? == 2, "the owner's exit did not release tty3")
    })
}

pub fn vt_switch_vt_process_competing() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (ready, go) = (Gate::new()?, Gate::new()?);
        let mut owner = owner_on_tty3(&ready, |tty3| {
            require(signalled(&RELEASES, 1, 3000), "no release signal")?;
            go.wait(3000, "the second request")?;
            tty3.ok("VT_RELDISP(1)", VT_RELDISP, 1)?;
            require(RELEASES.load(Ordering::SeqCst) == 1, "the second request signalled the owner again")
        })?;
        activate(2)?;
        activate(4)?;
        go.open();
        wait_active(4)?;
        owner.finish(3000)?;
        require(foreground()? == 4, "the earlier request won")
    })
}

pub fn vt_switch_vt_process_cancel() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (ready, cancelled) = (Gate::new()?, Gate::new()?);
        let mut owner = owner_on_tty3(&ready, |tty3| {
            require(signalled(&RELEASES, 1, 3000), "no release signal")?;
            cancelled.wait(3000, "the cancellation")?;
            require(signalled(&ACQUIRES, 1, 3000), "the cancellation sent no acquire signal")?;
            tty3.fails("VT_RELDISP(1) after the cancellation", VT_RELDISP, 1, EINVAL)
        })?;
        activate(2)?;
        sleep(100);
        activate(3)?;
        cancelled.open();
        owner.finish(4000)?;
        require(foreground()? == 3, "the cancelled switch moved the foreground")
    })
}

pub fn vt_switch_setmode_busy() -> CaseResult {
    measured(|| {
        let ready = Gate::new()?;
        let done = Gate::new()?;
        let mut owner = Child::spawn(|| {
            own(&Term::vt(3)?)?;
            ready.open();
            done.wait(5000, "the case")
        })?;
        ready.wait(3000, "the owner")?;
        let tty3 = Term::vt(3)?;
        let process = vt_mode(VT_PROCESS, signal::SIGUSR1 as i16, signal::SIGUSR2 as i16);
        tty3.fails("VT_SETMODE(VT_PROCESS) on an owned terminal", VT_SETMODE, process.as_ptr() as u64, EBUSY)?;
        tty3.fails("VT_SETMODE(VT_AUTO) from another process", VT_SETMODE, vt_mode(VT_AUTO, 0, 0).as_ptr() as u64, EPERM)?;
        let mut mode = [0u8; 8];
        tty3.ok("VT_GETMODE", VT_GETMODE, mode.as_mut_ptr() as u64)?;
        require(mode[0] == VT_PROCESS && i16::from_le_bytes([mode[2], mode[3]]) == signal::SIGUSR1 as i16, "VT_GETMODE")?;
        done.open();
        owner.finish(3000)?;
        tty3.ok("VT_SETMODE once the owner exited", VT_SETMODE, process.as_ptr() as u64)?;
        tty3.fails("VT_SETMODE mode 2", VT_SETMODE, vt_mode(2, 10, 12).as_ptr() as u64, EINVAL)?;
        tty3.fails("VT_SETMODE signal 0", VT_SETMODE, vt_mode(VT_PROCESS, 0, 12).as_ptr() as u64, EINVAL)?;
        tty3.ok("VT_SETMODE(VT_AUTO)", VT_SETMODE, vt_mode(VT_AUTO, 0, 0).as_ptr() as u64).map(drop)
    })
}

pub fn vt_switch_reldisp_errors() -> CaseResult {
    measured(|| {
        let tty3 = Term::vt(3)?;
        let process = vt_mode(VT_PROCESS, signal::SIGUSR1 as i16, signal::SIGUSR2 as i16);
        tty3.ok("VT_SETMODE(VT_PROCESS)", VT_SETMODE, process.as_ptr() as u64)?;
        let result = (|| {
            tty3.fails("VT_RELDISP(1) with no release pending", VT_RELDISP, 1, EINVAL)?;
            tty3.fails("VT_RELDISP(0) with no release pending", VT_RELDISP, 0, EINVAL)?;
            tty3.fails("VT_ACKACQ with no acquisition pending", VT_RELDISP, VT_ACKACQ, EINVAL)?;
            Child::spawn(|| Term::vt(3)?.fails("VT_RELDISP from another process", VT_RELDISP, 1, EPERM))?.finish(3000)
        })();
        tty3.ok("VT_SETMODE(VT_AUTO)", VT_SETMODE, vt_mode(VT_AUTO, 0, 0).as_ptr() as u64)?;
        result
    })
}

pub fn vt_switch_eperm_unprivileged() -> CaseResult {
    measured(|| {
        Child::spawn(|| {
            libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
            #[cfg(target_arch = "x86_64")]
            const SETUID: u64 = 105;
            #[cfg(target_arch = "aarch64")]
            const SETUID: u64 = 146;
            // SAFETY: setuid takes its user ID by value.
            require(unsafe { raw::syscall1(SETUID, 1000) } as i64 == 0, "setuid(1000)")?;
            let tty2 = Term::vt(2)?;
            tty2.fails("VT_ACTIVATE", VT_ACTIVATE, 3, EPERM)?;
            tty2.fails("KDSETMODE", KDSETMODE, KD_TEXT, EPERM)?;
            let mode = vt_mode(VT_PROCESS, 10, 12);
            tty2.fails("VT_SETMODE", VT_SETMODE, mode.as_ptr() as u64, EPERM)?;
            let state = tty2.state()?;
            tty2.ok("VT_WAITACTIVE of the foreground", VT_WAITACTIVE, u64::from(state[0]))?;
            let mut kd = -1i32;
            tty2.ok("KDGETMODE", KDGETMODE, &mut kd as *mut i32 as u64).map(drop)
        })?
        .finish(3000)?;
        require(foreground()? == 1, "an unprivileged request switched terminals")
    })
}

/// The 16 hex digits a switch line gives for terminal `n`, the last one logged.
fn switch_digest(log: &str, n: usize) -> Option<String> {
    let prefix = format!("[console] tty{n} shown: read back identical; digest ");
    log.lines().rev().find_map(|line| {
        line.split_once(&prefix).map(|(_, rest)| rest.split(';').next().unwrap_or("").trim().to_string())
    })
}

/// How long after the switch the last read-back line for terminal `n` was
/// logged, in milliseconds (`; <ms> ms after the switch`).
fn switch_ms(log: &str, n: usize) -> Option<f64> {
    let prefix = format!("[console] tty{n} shown: read back identical; digest ");
    log.lines().rev().find_map(|line| {
        let (_, rest) = line.split_once(&prefix)?;
        rest.split("; ").nth(1)?.strip_suffix(" ms after the switch")?.trim().parse().ok()
    })
}

pub fn vt_switch_switch_readback() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let label = mark("readback");
        Term::vt(2)?.write(format!("\x1b[H\x1b[2J{label} Wm09!?\r\n").as_bytes())?;
        let shown_line = "[console] tty2 shown: read back identical; digest ";
        let before = count(&kmsg()?, shown_line);
        switch_to(2)?;
        shows(2, &label)?;
        require(within(2000, || count(&kmsg().unwrap_or_default(), shown_line) == before + 1), "the switch was not read back identical")?;
        let log = kmsg()?;
        require(!log.contains("[console] tty2 shown: read back differs"), "a read-back differed")?;
        // Plan Q25: the switch is read back within this case's 2000 ms.
        let ms = switch_ms(&log, 2).ok_or("the read-back line gives no time after the switch")?;
        require(ms < 2000.0, &format!("read back {ms} ms after the switch, over 2000 ms"))
    })
}

pub fn vt_switch_switch_log_digest() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let secret = mark("secret");
        Term::vt(6)?.write(format!("\x1b[H\x1b[2J{secret}\r\n").as_bytes())?;
        switch_to(6)?;
        shows(6, &secret)?;
        let mut log = String::new();
        require(within(2000, || {
            log = kmsg().unwrap_or_default();
            switch_digest(&log, 6).is_some()
        }), "no read-back line for tty6")?;
        let digest = switch_digest(&log, 6).unwrap_or_default();
        require(digest.len() == 16 && digest.bytes().all(|b| b.is_ascii_hexdigit()), &format!("digest {digest:?}"))?;
        let check = screen_check()?;
        let sha = check.lines().find_map(|l| l.strip_prefix("console sha256: ")).ok_or("no sha256")?;
        require(sha.starts_with(&digest), &format!("digest {digest} is not the shadow's sha256 {sha}"))?;
        require(log.contains("[console] foreground tty6 (ioctl)"), "no foreground line for tty6")?;
        require(!log.contains(&secret), "the switch logged the screen's text")
    })
}

// ---------------------------------------------------------------------------
// scrollback
// ---------------------------------------------------------------------------

/// Clear terminal `n` and write `lines` numbered lines to it; its row count.
fn fill(term: &Term, label: &str, lines: usize) -> Verdict<usize> {
    let rows = usize::from(term.winsize()?[0]);
    let mut text = String::from("\x1b[H\x1b[2J");
    for k in 0..lines {
        text.push_str(&format!("{label}-{k:03}\r\n"));
    }
    term.write(text.as_bytes())?;
    Ok(rows)
}

/// The rows shown, waiting for the screen to show terminal `n` with `marker`.
fn view(n: usize, marker: &str) -> Verdict<Vec<String>> {
    shows(n, marker)
}

/// Returns a terminal's view to its live screen when a case ends, however it
/// ends, so a failed scrollback case leaves no scrolled-back view to later cases.
struct LiveView<'a>(&'a Term);

impl Drop for LiveView<'_> {
    fn drop(&mut self) {
        let _ = self.0.scroll(-1_000_000);
    }
}

pub fn scrollback_history_kept() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty2 = Term::vt(2)?;
        let _live = LiveView(&tty2);
        let label = mark("hist");
        let rows = fill(&tty2, &label, 200)?;
        switch_to(2)?;
        // The live screen's first line is the one `rows - 1` before the end.
        let top = 200 - (rows - 1);
        view(2, &format!("{label}-199"))?;
        require(tty2.scroll(rows as i64)? == rows as i64, "the view did not move back a screen")?;
        let older = view(2, &format!("{label}-{:03}", top - 1))?;
        require(!older.iter().any(|row| row.contains(&format!("{label}-199"))), "the scrolled-back view still shows the last line")?;
        require(tty2.scroll(-(rows as i64))? == 0, "the view did not return")?;
        view(2, &format!("{label}-199")).map(drop)
    })
}

pub fn scrollback_history_per_vt() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (tty2, tty3) = (Term::vt(2)?, Term::vt(3)?);
        let _live = (LiveView(&tty2), LiveView(&tty3));
        let (two, three) = (mark("per-2"), mark("per-3"));
        let rows = fill(&tty2, &two, 150)?;
        fill(&tty3, &three, 150)?;
        for (n, term, label, other) in [(2, &tty2, &two, &three), (3, &tty3, &three, &two)] {
            switch_to(n)?;
            term.scroll(rows as i64)?;
            let older = view(n, &format!("{label}-{:03}", 150 - rows))?;
            require(!older.iter().any(|row| row.contains(other.as_str())), &format!("tty{n}'s scrollback shows another terminal's lines"))?;
            term.scroll(-(rows as i64))?;
        }
        Ok(())
    })
}

pub fn scrollback_survives_switch() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty2 = Term::vt(2)?;
        let _live = LiveView(&tty2);
        let label = mark("survive");
        let rows = fill(&tty2, &label, 150)?;
        switch_to(2)?;
        tty2.scroll(rows as i64)?;
        let back = format!("{label}-{:03}", 150 - rows);
        let before = view(2, &back)?;
        switch_to(3)?;
        switch_to(2)?;
        require(tty2.scroll(0)? == rows as i64, "the view position did not survive the switch")?;
        let after = view(2, &back)?;
        tty2.scroll(-(rows as i64))?;
        require(before == after, "the scrolled-back view changed across the switches")
    })
}

pub fn scrollback_view_holds_on_output() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty2 = Term::vt(2)?;
        let _live = LiveView(&tty2);
        let label = mark("hold");
        let rows = fill(&tty2, &label, 150)?;
        switch_to(2)?;
        tty2.scroll(rows as i64)?;
        let back = format!("{label}-{:03}", 150 - rows);
        let before = view(2, &back)?;
        let newer = mark("newer");
        tty2.write(format!("{newer}-1\r\n{newer}-2\r\n").as_bytes())?;
        sleep(200);
        let after = view(2, &back)?;
        require(before == after, "new output moved the scrolled-back view")?;
        require(tty2.scroll(0)? == rows as i64 + 2, "the view did not keep its lines as output arrived")?;
        tty2.scroll(-1000)?;
        view(2, &format!("{newer}-2")).map(drop)
    })
}

pub fn scrollback_key_returns_live() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty2 = Term::vt(2)?;
        let _live = LiveView(&tty2);
        let label = mark("key-live");
        let rows = fill(&tty2, &label, 150)?;
        switch_to(2)?;
        tty2.scroll(rows as i64)?;
        view(2, &format!("{label}-{:03}", 150 - rows))?;
        libbreenix::suite::hostkeys("a")?;
        let live = within(3000, || tty2.scroll(0).is_ok_and(|back| back == 0));
        let _ = tty2.ok("TCFLSH", TCFLSH, 0);
        require(live, "a key did not return the view to the live screen")?;
        view(2, &format!("{label}-149")).map(drop)
    })
}
