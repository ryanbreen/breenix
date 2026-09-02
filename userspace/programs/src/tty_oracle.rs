//! Production-profile TTY evidence leg (green program, arc 4).
//!
//! Every other TTY proof in the tree runs inside the kernel's `boot_tests`
//! registry, so it measures a kernel that is not the one shipped. This oracle
//! is a plain userspace program launched from init, which means it exercises
//! the PTY and line-discipline surface through real syscalls on the exact
//! kernel `scripts/parallels/build-efi.sh` deploys -- there is no feature flag,
//! no injection seam and no kernel-side test byte behind it.
//!
//! Every arm is a two-sided claim: each one contains its own contrast, so a
//! dead path cannot be mistaken for a passing one. Canonical mode withholds an
//! unterminated line AND releases the same bytes once the newline lands; raw
//! mode delivers the same unterminated bytes immediately; ONLCR rewrites `\n`
//! and stops rewriting it when OPOST is cleared. A path that silently did
//! nothing would fail the second half of its own arm.
//!
//! Safety: the oracle must never wedge a boot. Both fds are opened
//! `O_NONBLOCK`, and the flag is asserted present on the fd (via `F_GETFL`)
//! before any read is attempted -- so if the kernel ever stops honouring
//! open-time `O_NONBLOCK` again, this program fails fast on the assertion
//! instead of blocking forever inside `read()`.

use libbreenix::error::Error;
use libbreenix::fs;
use libbreenix::io;
use libbreenix::io::fd_flags::FD_CLOEXEC;
use libbreenix::process;
use libbreenix::process::ForkResult;
use libbreenix::pty;
use libbreenix::termios::{self, cc, iflag, lflag, oflag, Termios, Winsize, TCSANOW};
use libbreenix::types::Fd;
use libbreenix::Errno;
use std::env;

/// `O_NONBLOCK` as `open(2)`/`posix_openpt(3)` take it.
const O_NONBLOCK_OPEN: i32 = 0x800;
/// The same bit as `fs::open` wants it.
const O_NONBLOCK_FS: u32 = 0x800;
/// A file that is definitely not a terminal -- created by `create_ext2_disk.sh`.
const NON_TTY_PATH: &str = "/etc/passwd";

struct Failure {
    arm: &'static str,
    detail: String,
}

fn fail(arm: &'static str, detail: String) -> Failure {
    Failure { arm, detail }
}

/// Emitted twice: console output interleaves at byte granularity, so a single
/// shredded copy must not be able to hide a verdict.
fn emit(line: &str) {
    print!("{}\n", line);
    print!("{}\n", line);
}

fn pass(arm: &str, detail: &str) {
    emit(&format!("[TTY_ORACLE:{}:verdict=PASS:{}]", arm, detail));
}

/// Render bytes so a failure detail is readable in a serial log.
fn show(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        match b {
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{:02x}", b)),
        }
    }
    out
}

/// A read that must find nothing. Returns `Ok(())` only on `EAGAIN` -- any
/// other errno (EIO, EBADF, ...) is itself a failure, not a pass. The arms
/// that call this (`nonblock_open`'s both_eagain=1, `hangup`'s
/// eagain_while_open=1, #704's ctty idle-read) print those claims into the
/// durable evidence, so the check has to mean exactly what it says: EAGAIN
/// specifically, not merely "did not return data".
fn expect_empty(fd: Fd, arm: &'static str, what: &str) -> Result<(), Failure> {
    let mut buf = [0u8; 64];
    match io::read(fd, &mut buf) {
        Ok(n) => Err(fail(
            arm,
            format!("{}=unexpected_data:n={}:bytes={}", what, n, show(&buf[..n])),
        )),
        Err(Error::Os(Errno::EAGAIN)) => Ok(()),
        Err(e) => Err(fail(arm, format!("{}=wrong_errno:want=EAGAIN:got={}", what, e))),
    }
}

/// A read that must find exactly `want`.
fn expect_bytes(fd: Fd, want: &[u8], arm: &'static str, what: &str) -> Result<(), Failure> {
    let mut buf = [0u8; 128];
    match io::read(fd, &mut buf) {
        Ok(n) if &buf[..n] == want => Ok(()),
        Ok(n) => Err(fail(
            arm,
            format!(
                "{}=wrong_bytes:want={}:got={}",
                what,
                show(want),
                show(&buf[..n])
            ),
        )),
        Err(e) => Err(fail(arm, format!("{}=read_failed:{}", what, e))),
    }
}

/// Discard anything pending so one arm cannot contaminate the next.
fn drain(fd: Fd) {
    let mut buf = [0u8; 256];
    for _ in 0..8 {
        match io::read(fd, &mut buf) {
            Ok(n) if n > 0 => continue,
            _ => break,
        }
    }
}

fn write_all(fd: Fd, data: &[u8], arm: &'static str, what: &str) -> Result<(), Failure> {
    match io::write(fd, data) {
        Ok(n) if n == data.len() => Ok(()),
        Ok(n) => Err(fail(arm, format!("{}=short_write:n={}", what, n))),
        Err(e) => Err(fail(arm, format!("{}=write_failed:{}", what, e))),
    }
}

fn get_termios(fd: Fd, arm: &'static str) -> Result<Termios, Failure> {
    let mut t = Termios::default();
    match termios::tcgetattr(fd, &mut t) {
        Ok(()) => Ok(t),
        Err(e) => Err(fail(arm, format!("tcgetattr_failed:{}", e))),
    }
}

fn set_termios(fd: Fd, t: &Termios, arm: &'static str) -> Result<(), Failure> {
    match termios::tcsetattr(fd, TCSANOW, t) {
        Ok(()) => Ok(()),
        Err(e) => Err(fail(arm, format!("tcsetattr_failed:{}", e))),
    }
}

/// Arm 1 -- the PTY allocation handshake, end to end.
fn arm_openpt() -> Result<(Fd, String), Failure> {
    const ARM: &str = "openpt";
    let master = match pty::posix_openpt(pty::O_RDWR | pty::O_NOCTTY | O_NONBLOCK_OPEN) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("posix_openpt_failed:{}", e))),
    };
    if let Err(e) = pty::grantpt(master) {
        return Err(fail(ARM, format!("grantpt_failed:{}", e)));
    }

    // The slave must not be openable before unlockpt(). This is the arm's
    // contrast: it proves the lock is enforced rather than merely recorded.
    let mut probe = [0u8; 32];
    let locked_len = match pty::ptsname(master, &mut probe) {
        Ok(len) => len,
        Err(e) => return Err(fail(ARM, format!("ptsname_before_unlock_failed:{}", e))),
    };
    let locked_path = String::from_utf8_lossy(&probe[..locked_len]).into_owned();
    if let Ok(fd) = fs::open(&locked_path, fs::O_RDWR | O_NONBLOCK_FS) {
        let _ = io::close(fd);
        return Err(fail(ARM, "locked_slave_was_openable".to_string()));
    }

    if let Err(e) = pty::unlockpt(master) {
        return Err(fail(ARM, format!("unlockpt_failed:{}", e)));
    }
    if !locked_path.starts_with("/dev/pts/") {
        return Err(fail(ARM, format!("bad_ptsname:{}", locked_path)));
    }

    pass(
        ARM,
        &format!("slave={}:locked_open_refused=1", locked_path),
    );
    Ok((master, locked_path))
}

/// Arm 2 -- open-time `O_NONBLOCK` reaches the fd on both PTY ends.
///
/// This is the arm that measures the defect this branch fixes. Both PTY open
/// paths used to discard the caller's flags, so the non-blocking branch of the
/// read handlers was reachable only through `fcntl(F_SETFL)`. The flag is
/// checked on the fd *before* the reads, so a regression reports a wrong flag
/// word rather than parking the boot inside `read()` forever.
fn arm_nonblock(master: Fd, slave_path: &str) -> Result<Fd, Failure> {
    const ARM: &str = "nonblock_open";

    let master_fl = match io::fcntl_getfl(master) {
        Ok(fl) => fl,
        Err(e) => return Err(fail(ARM, format!("master_getfl_failed:{}", e))),
    };
    if master_fl & (O_NONBLOCK_OPEN as i64) == 0 {
        return Err(fail(
            ARM,
            format!("master_nonblock_dropped:fl={:#x}", master_fl),
        ));
    }

    let slave = match fs::open(slave_path, fs::O_RDWR | O_NONBLOCK_FS) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("slave_open_failed:{}", e))),
    };
    let slave_fl = match io::fcntl_getfl(slave) {
        Ok(fl) => fl,
        Err(e) => return Err(fail(ARM, format!("slave_getfl_failed:{}", e))),
    };
    if slave_fl & (O_NONBLOCK_OPEN as i64) == 0 {
        let _ = io::close(slave);
        return Err(fail(
            ARM,
            format!("slave_nonblock_dropped:fl={:#x}", slave_fl),
        ));
    }

    // Both ends are idle, so both reads must report EAGAIN instead of parking.
    if let Err(f) = expect_empty(master, ARM, "master_idle_read") {
        let _ = io::close(slave);
        return Err(f);
    }
    if let Err(f) = expect_empty(slave, ARM, "slave_idle_read") {
        let _ = io::close(slave);
        return Err(f);
    }

    pass(
        ARM,
        &format!(
            "master_fl={:#x}:slave_fl={:#x}:both_eagain=1",
            master_fl, slave_fl
        ),
    );
    Ok(slave)
}

/// Arm 3 -- `isatty` separates a PTY from a regular file.
fn arm_isatty(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "isatty";
    if !termios::isatty(master) {
        return Err(fail(ARM, "master_not_a_tty".to_string()));
    }
    if !termios::isatty(slave) {
        return Err(fail(ARM, "slave_not_a_tty".to_string()));
    }
    let regular = match fs::open(NON_TTY_PATH, fs::O_RDONLY) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("open_regular_failed:{}", e))),
    };
    let regular_is_tty = termios::isatty(regular);
    let _ = io::close(regular);
    if regular_is_tty {
        return Err(fail(ARM, format!("{}_reported_as_tty", NON_TTY_PATH)));
    }
    pass(ARM, "master=1:slave=1:regular_file=0");
    Ok(())
}

/// Arm 4 -- termios is real state, not a stub that answers with defaults.
fn arm_termios(slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "termios_roundtrip";
    let original = get_termios(slave, ARM)?;
    if original.c_lflag & lflag::ICANON == 0 || original.c_lflag & lflag::ECHO == 0 {
        return Err(fail(
            ARM,
            format!("default_not_canonical_echo:lflag={:#x}", original.c_lflag),
        ));
    }
    if original.c_oflag & oflag::OPOST == 0 || original.c_iflag & iflag::ICRNL == 0 {
        return Err(fail(
            ARM,
            format!(
                "default_flags_missing:iflag={:#x}:oflag={:#x}",
                original.c_iflag, original.c_oflag
            ),
        ));
    }

    let mut modified = original;
    modified.c_lflag &= !(lflag::ECHO | lflag::ECHOE);
    modified.c_iflag &= !iflag::ICRNL;
    modified.c_cc[cc::VMIN] = 4;
    modified.c_cc[cc::VTIME] = 7;
    modified.c_cc[cc::VINTR] = 0x05;
    set_termios(slave, &modified, ARM)?;

    let read_back = get_termios(slave, ARM)?;
    if read_back.c_lflag != modified.c_lflag
        || read_back.c_iflag != modified.c_iflag
        || read_back.c_cc[cc::VMIN] != 4
        || read_back.c_cc[cc::VTIME] != 7
        || read_back.c_cc[cc::VINTR] != 0x05
    {
        return Err(fail(
            ARM,
            format!(
                "readback_mismatch:lflag={:#x}/{:#x}:iflag={:#x}/{:#x}:vmin={}:vtime={}:vintr={:#x}",
                read_back.c_lflag,
                modified.c_lflag,
                read_back.c_iflag,
                modified.c_iflag,
                read_back.c_cc[cc::VMIN],
                read_back.c_cc[cc::VTIME],
                read_back.c_cc[cc::VINTR]
            ),
        ));
    }

    // Restore, and prove the restore also took -- a one-way setter would pass
    // the check above and still be broken.
    set_termios(slave, &original, ARM)?;
    let restored = get_termios(slave, ARM)?;
    if restored.c_lflag != original.c_lflag || restored.c_cc[cc::VINTR] != original.c_cc[cc::VINTR] {
        return Err(fail(
            ARM,
            format!("restore_failed:lflag={:#x}", restored.c_lflag),
        ));
    }

    pass(
        ARM,
        &format!(
            "default_lflag={:#x}:modified_lflag={:#x}:restored=1",
            original.c_lflag, modified.c_lflag
        ),
    );
    Ok(())
}

/// Arm 5 -- canonical mode holds an unterminated line and releases it on `\n`.
fn arm_canonical(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "canonical_line";
    let mut t = get_termios(slave, ARM)?;
    t.c_lflag |= lflag::ICANON;
    t.c_lflag &= !lflag::ECHO; // keep the master side clean for this arm
    set_termios(slave, &t, ARM)?;
    drain(master);
    drain(slave);

    write_all(master, b"hello", ARM, "partial_line")?;
    // The bytes are inside the line discipline, not the slave's read queue.
    expect_empty(slave, ARM, "unterminated_read")?;

    write_all(master, b"\n", ARM, "terminator")?;
    expect_bytes(slave, b"hello\n", ARM, "completed_line")?;

    pass(ARM, "withheld_unterminated=1:delivered_on_newline=6");
    Ok(())
}

/// Arm 6 -- ICRNL maps a carriage return to a line terminator on input.
fn arm_icrnl(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "icrnl";
    let mut t = get_termios(slave, ARM)?;
    t.c_lflag |= lflag::ICANON;
    t.c_lflag &= !lflag::ECHO;
    t.c_iflag |= iflag::ICRNL;
    set_termios(slave, &t, ARM)?;
    drain(master);
    drain(slave);

    // A bare CR must terminate the line, and arrive as NL.
    write_all(master, b"cr\r", ARM, "cr_line")?;
    expect_bytes(slave, b"cr\n", ARM, "cr_translated")?;

    // With ICRNL cleared the same CR must no longer terminate anything.
    t.c_iflag &= !iflag::ICRNL;
    set_termios(slave, &t, ARM)?;
    write_all(master, b"raw\r", ARM, "cr_untranslated")?;
    expect_empty(slave, ARM, "cr_no_longer_terminates")?;

    // Release the held line so it cannot leak into the next arm.
    write_all(master, b"\n", ARM, "release")?;
    drain(slave);
    t.c_iflag |= iflag::ICRNL;
    set_termios(slave, &t, ARM)?;

    pass(ARM, "cr_to_nl=1:cleared_icrnl_holds=1");
    Ok(())
}

/// Arm 7 -- raw mode delivers the very bytes canonical mode withheld.
fn arm_raw(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "raw_passthrough";
    let mut t = get_termios(slave, ARM)?;
    t.c_lflag &= !(lflag::ICANON | lflag::ECHO);
    t.c_cc[cc::VMIN] = 1;
    t.c_cc[cc::VTIME] = 0;
    set_termios(slave, &t, ARM)?;
    drain(master);
    drain(slave);

    // No newline anywhere: in arm 5 this produced EAGAIN.
    write_all(master, b"xy", ARM, "unterminated")?;
    expect_bytes(slave, b"xy", ARM, "immediate_delivery")?;

    pass(ARM, "unterminated_delivered=2");
    Ok(())
}

/// Arm 8 -- ECHO returns input to the master side, and stops when cleared.
fn arm_echo(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "echo";
    let mut t = get_termios(slave, ARM)?;
    t.c_lflag |= lflag::ICANON | lflag::ECHO;
    set_termios(slave, &t, ARM)?;
    drain(master);
    drain(slave);

    write_all(master, b"ab\n", ARM, "echoed_line")?;
    let mut buf = [0u8; 64];
    let echoed = match io::read(master, &mut buf) {
        Ok(n) => n,
        Err(e) => return Err(fail(ARM, format!("echo_read_failed:{}", e))),
    };
    if !buf[..echoed].contains(&b'a') || !buf[..echoed].contains(&b'b') {
        return Err(fail(
            ARM,
            format!("echo_missing:got={}", show(&buf[..echoed])),
        ));
    }
    drain(slave);

    // Clearing ECHO must silence it -- otherwise the arm above proves nothing.
    t.c_lflag &= !lflag::ECHO;
    set_termios(slave, &t, ARM)?;
    drain(master);
    write_all(master, b"cd\n", ARM, "silent_line")?;
    expect_empty(master, ARM, "echo_off_read")?;
    drain(slave);

    pass(ARM, &format!("echo_on_bytes={}:echo_off_bytes=0", echoed));
    Ok(())
}

/// Arm 9 -- output post-processing (`OPOST`/`ONLCR`) on the slave->master path.
fn arm_onlcr(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "onlcr";
    let mut t = get_termios(slave, ARM)?;
    t.c_oflag |= oflag::OPOST | oflag::ONLCR;
    set_termios(slave, &t, ARM)?;
    drain(master);

    write_all(slave, b"a\n", ARM, "onlcr_write")?;
    expect_bytes(master, b"a\r\n", ARM, "onlcr_translated")?;

    // With OPOST cleared the same write must pass through untouched.
    t.c_oflag &= !oflag::OPOST;
    set_termios(slave, &t, ARM)?;
    write_all(slave, b"b\n", ARM, "opost_off_write")?;
    expect_bytes(master, b"b\n", ARM, "opost_off_untranslated")?;

    t.c_oflag |= oflag::OPOST | oflag::ONLCR;
    set_termios(slave, &t, ARM)?;
    pass(ARM, "onlcr_expanded=3:opost_off_passthrough=2");
    Ok(())
}

/// Arm 10 -- window size is stored per-PTY and read back.
fn arm_winsize(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "winsize";
    let before = match termios::get_winsize(master) {
        Ok(ws) => ws,
        Err(e) => return Err(fail(ARM, format!("get_winsize_failed:{}", e))),
    };
    let want = Winsize {
        ws_row: 41,
        ws_col: 133,
        ws_xpixel: 7,
        ws_ypixel: 11,
    };
    if before.ws_row == want.ws_row && before.ws_col == want.ws_col {
        return Err(fail(ARM, "default_already_equals_probe".to_string()));
    }
    if let Err(e) = termios::set_winsize(master, &want) {
        return Err(fail(ARM, format!("set_winsize_failed:{}", e)));
    }
    // Read it back from the *other* end: the size belongs to the PTY pair, not
    // to whichever fd set it.
    let got = match termios::get_winsize(slave) {
        Ok(ws) => ws,
        Err(e) => return Err(fail(ARM, format!("slave_get_winsize_failed:{}", e))),
    };
    if got.ws_row != want.ws_row
        || got.ws_col != want.ws_col
        || got.ws_xpixel != want.ws_xpixel
        || got.ws_ypixel != want.ws_ypixel
    {
        return Err(fail(
            ARM,
            format!(
                "readback_mismatch:rows={}:cols={}:x={}:y={}",
                got.ws_row, got.ws_col, got.ws_xpixel, got.ws_ypixel
            ),
        ));
    }
    pass(
        ARM,
        &format!(
            "default={}x{}:set={}x{}:crossed_ends=1",
            before.ws_row, before.ws_col, got.ws_row, got.ws_col
        ),
    );
    Ok(())
}

/// Arm 11 -- the foreground process group round-trips through the PTY.
fn arm_pgrp(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "foreground_pgrp";
    let pid = match process::getpid() {
        Ok(p) => p.raw() as u32,
        Err(e) => return Err(fail(ARM, format!("getpid_failed:{}", e))),
    };
    if let Err(e) = termios::tcsetpgrp(master, pid as i32) {
        return Err(fail(ARM, format!("tcsetpgrp_failed:{}", e)));
    }
    match termios::tcgetpgrp(slave) {
        Ok(got) if got as u32 == pid => {}
        Ok(got) => {
            return Err(fail(ARM, format!("readback_mismatch:want={}:got={}", pid, got)))
        }
        Err(e) => return Err(fail(ARM, format!("tcgetpgrp_failed:{}", e))),
    }
    pass(ARM, &format!("pgrp={}:crossed_ends=1", pid));
    Ok(())
}

/// Arm 12 -- closing the last slave fd hangs the master up (read returns EOF).
fn arm_hangup(master: Fd, slave: Fd) -> Result<(), Failure> {
    const ARM: &str = "hangup";
    drain(master);
    // While the slave is open an idle master reports EAGAIN, never EOF.
    expect_empty(master, ARM, "pre_close_read")?;

    if io::close(slave).is_err() {
        return Err(fail(ARM, "slave_close_failed".to_string()));
    }

    let mut buf = [0u8; 32];
    match io::read(master, &mut buf) {
        Ok(0) => {}
        Ok(n) => {
            return Err(fail(
                ARM,
                format!("expected_eof_got_data:n={}:bytes={}", n, show(&buf[..n])),
            ))
        }
        Err(e) => return Err(fail(ARM, format!("expected_eof_got_error:{}", e))),
    }
    pass(ARM, "eagain_while_open=1:eof_after_close=1");
    Ok(())
}

/// Arm 13 -- #704: `/dev/tty` must account for its slave fd exactly like
/// `/dev/pts/N` does, or the master hangs up while a slave fd is still open.
///
/// The other eleven arms deliberately open `O_NOCTTY` and never establish a
/// controlling terminal (#705's gap). This arm is self-contained -- its own
/// PTY pair, its own session -- so it can drive `setsid()`, `TIOCSCTTY` and
/// `/dev/tty` for real without disturbing the shared master/slave the other
/// arms use.
///
/// Sequence and why each step is there:
/// 1. Open a `/dev/pts/N` slave first. That path's `slave_open()` accounting
///    is not in question; it establishes the refcount this arm then watches.
/// 2. `setsid()` + `TIOCSCTTY` on that slave, then open `/dev/tty`. It must
///    resolve to the SAME PTY -- proven by writing a marker through the
///    `/dev/tty` fd and reading it back on the master, which only a real
///    alias of this pty (not the no-ctty fallback device) could deliver.
/// 3. Close the `/dev/tty` fd immediately. On the unfixed kernel this is the
///    exact moment #704 fires: the alias's open never called `slave_open()`,
///    so this close's unconditional `slave_close()` drops the shared count
///    to zero while the original `/dev/pts/N` fd is still open.
/// 4. An IDLE master read, with nothing written and nothing pending, must
///    report EAGAIN rather than EOF. This is the assertion that actually
///    distinguishes fixed from broken: `master_read()` drains any buffered
///    data ahead of its hangup check, so a write-then-read alone would pass
///    on the broken kernel too -- the bytes still arrive even after the
///    refcount has wrongly hit zero.
/// 5. A functional write/read round trip confirms the survivor is not
///    merely "not yet flagged hung up" but genuinely live.
/// 6. Close the one remaining slave fd and require EOF now, not before.
fn arm_ctty() -> Result<(), Failure> {
    const ARM: &str = "ctty";

    let master = match pty::posix_openpt(pty::O_RDWR | pty::O_NOCTTY | O_NONBLOCK_OPEN) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("posix_openpt_failed:{}", e))),
    };
    if let Err(e) = pty::grantpt(master) {
        return Err(fail(ARM, format!("grantpt_failed:{}", e)));
    }
    if let Err(e) = pty::unlockpt(master) {
        return Err(fail(ARM, format!("unlockpt_failed:{}", e)));
    }
    let master_fl = match io::fcntl_getfl(master) {
        Ok(fl) => fl,
        Err(e) => return Err(fail(ARM, format!("master_getfl_failed:{}", e))),
    };
    if master_fl & (O_NONBLOCK_OPEN as i64) == 0 {
        return Err(fail(
            ARM,
            format!("master_nonblock_dropped:fl={:#x}", master_fl),
        ));
    }

    let mut path_buf = [0u8; 32];
    let path_len = match pty::ptsname(master, &mut path_buf) {
        Ok(len) => len,
        Err(e) => return Err(fail(ARM, format!("ptsname_failed:{}", e))),
    };
    let slave_path = String::from_utf8_lossy(&path_buf[..path_len]).into_owned();

    let slave = match fs::open(&slave_path, fs::O_RDWR | O_NONBLOCK_FS) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("slave_open_failed:{}", e))),
    };

    // The oracle is freshly exec'd by init; setsid() succeeds unconditionally
    // on this kernel regardless of the caller's current process-group state
    // (kernel/src/syscall/session.rs never actually returns EPERM). This
    // makes the calling process its own session leader: sid == pgid == pid,
    // which is what handle_devfs_open's /dev/tty arm matches on below.
    if let Err(e) = process::setsid() {
        return Err(fail(ARM, format!("setsid_failed:{}", e)));
    }

    if let Err(e) = termios::set_controlling_terminal(slave) {
        return Err(fail(ARM, format!("tiocsctty_failed:{}", e)));
    }

    let ttyfd = match fs::open("/dev/tty", fs::O_RDWR | O_NONBLOCK_FS) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("dev_tty_open_failed:{}", e))),
    };

    // Prove /dev/tty is a real alias of THIS pty, not the no-ctty fallback
    // device: only a slave fd of this pty could deliver these bytes to the
    // master's read queue. Default oflag is OPOST|ONLCR, so LF becomes CRLF.
    write_all(ttyfd, b"ctty-alias\n", ARM, "alias_marker_write")?;
    expect_bytes(master, b"ctty-alias\r\n", ARM, "alias_marker_read")?;

    if io::close(ttyfd).is_err() {
        return Err(fail(ARM, "ctty_fd_close_failed".to_string()));
    }

    // #704's assertion. Checked with nothing written and nothing pending, so
    // the only way this sees data instead of EAGAIN is a bug; the only way
    // it sees EOF (Ok(0)) is the pair having been wrongly hung up already.
    if let Err(f) = expect_empty(master, ARM, "post_alias_close_idle_read") {
        return Err(fail(
            ARM,
            format!("slave_hung_up_after_alias_close:{}", f.detail),
        ));
    }

    // Functional confirmation the surviving /dev/pts/N slave is genuinely
    // live, not merely unchecked.
    write_all(slave, b"still-live\n", ARM, "post_alias_close_write")?;
    expect_bytes(master, b"still-live\r\n", ARM, "post_alias_close_read")?;

    if io::close(slave).is_err() {
        return Err(fail(ARM, "slave_close_failed".to_string()));
    }

    let mut buf = [0u8; 32];
    match io::read(master, &mut buf) {
        Ok(0) => {}
        Ok(n) => {
            return Err(fail(
                ARM,
                format!("expected_eof_got_data:n={}:bytes={}", n, show(&buf[..n])),
            ))
        }
        Err(e) => return Err(fail(ARM, format!("expected_eof_got_error:{}", e))),
    }

    let _ = io::close(master);
    pass(
        ARM,
        "dev_tty_aliased_slave=1:survived_alias_close=1:eof_after_last_slave_close=1",
    );
    Ok(())
}

/// Arm 14 -- #704-class: `close_cloexec()` (the exec() close-on-exec path)
/// must retire an `FdKind::PtySlave` fd exactly like `sys_close()` does, or a
/// slave fd marked `FD_CLOEXEC` that survives an `exec()` leaks the shared
/// refcount and the master can never observe hangup.
///
/// Found by this round's audit of every path that hands out or retires a
/// PTY-slave fd (the same audit that closed #704 at the `/dev/tty` arm):
/// `close_cloexec()` took every other release path's refcount accounting
/// (pipe/fifo/UnixStream) but routed `FdKind::PtySlave`/`FdKind::PtyMaster`
/// through its catch-all `_ => {}` arm, with no accounting at all. Fixed in
/// `kernel/src/ipc/fd.rs` alongside this arm, mirroring `sys_close`'s two
/// arms exactly.
///
/// Sequence: open a fresh, self-contained PTY pair, mark the slave
/// `FD_CLOEXEC` via `fcntl(F_SETFD)` (the route that predates round one's
/// `O_CLOEXEC`-on-open plumbing), `fork()`, and have the child `exec()` a
/// fresh copy of this very program with a marker argv (`--cloexec-child`)
/// that does nothing but `exit(0)` -- so the only kernel-side event that can
/// retire the child's cloned slave fd is `close_cloexec()` itself, not
/// anything the child does. The parent reaps the child, closes its OWN slave
/// fd, and then requires the master to see EOF: if `close_cloexec()` did its
/// job, the parent's close is the last live slave reference; if it did not,
/// the child's fork-cloned fd was never decremented and the master is stuck
/// reporting EAGAIN forever. Checked with an IDLE read, same framing as arm
/// 13 (`ctty`) and for the same reason: `master_read()` drains any buffered
/// data ahead of its hangup check, so a write-then-read would not discriminate.
fn arm_cloexec_exec() -> Result<(), Failure> {
    const ARM: &str = "cloexec_exec";

    let master = match pty::posix_openpt(pty::O_RDWR | pty::O_NOCTTY | O_NONBLOCK_OPEN) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("posix_openpt_failed:{}", e))),
    };
    if let Err(e) = pty::grantpt(master) {
        return Err(fail(ARM, format!("grantpt_failed:{}", e)));
    }
    if let Err(e) = pty::unlockpt(master) {
        return Err(fail(ARM, format!("unlockpt_failed:{}", e)));
    }

    let mut path_buf = [0u8; 32];
    let path_len = match pty::ptsname(master, &mut path_buf) {
        Ok(len) => len,
        Err(e) => return Err(fail(ARM, format!("ptsname_failed:{}", e))),
    };
    let slave_path = String::from_utf8_lossy(&path_buf[..path_len]).into_owned();

    let slave = match fs::open(&slave_path, fs::O_RDWR | O_NONBLOCK_FS) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(ARM, format!("slave_open_failed:{}", e))),
    };

    if let Err(e) = io::fcntl_setfd(slave, FD_CLOEXEC) {
        return Err(fail(ARM, format!("fcntl_setfd_failed:{}", e)));
    }
    match io::fcntl_getfd(slave) {
        Ok(flags) if flags & (FD_CLOEXEC as i64) != 0 => {}
        Ok(flags) => {
            return Err(fail(
                ARM,
                format!("cloexec_not_set_after_setfd:flags={:#x}", flags),
            ))
        }
        Err(e) => return Err(fail(ARM, format!("fcntl_getfd_failed:{}", e))),
    }

    let child_pid = match process::fork() {
        Ok(ForkResult::Child) => {
            // Child: exec a fresh copy of this program in marker mode. This
            // is the event under test -- close_cloexec() runs as part of
            // THIS exec(), on the child's fork-cloned slave fd.
            let path = b"/bin/tty_oracle\0";
            let arg0 = b"tty_oracle\0".as_ptr();
            let arg1 = b"--cloexec-child\0".as_ptr();
            let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];
            let _ = process::execv(path, argv.as_ptr());
            // exec() only returns on failure.
            process::exit(127);
        }
        Ok(ForkResult::Parent(pid)) => pid,
        Err(e) => return Err(fail(ARM, format!("fork_failed:{}", e))),
    };

    let mut status: i32 = 0;
    if let Err(e) = process::waitpid(child_pid.raw() as i32, &mut status, 0) {
        return Err(fail(ARM, format!("waitpid_failed:{}", e)));
    }
    if !process::wifexited(status) || process::wexitstatus(status) != 0 {
        return Err(fail(
            ARM,
            format!("child_did_not_exit_cleanly:status={:#x}", status),
        ));
    }

    // Close the parent's OWN slave fd. If close_cloexec() correctly retired
    // the child's fork-cloned copy during exec(), this is the last live
    // slave reference and the master must now see EOF.
    if io::close(slave).is_err() {
        return Err(fail(ARM, "slave_close_failed".to_string()));
    }

    // #704-class idle read: nothing written, nothing pending, so the only
    // way this sees data is a real bug and the only way it sees EAGAIN
    // instead of EOF is close_cloexec() having leaked the child's
    // fork-cloned slave reference across exec().
    let mut buf = [0u8; 32];
    match io::read(master, &mut buf) {
        Ok(0) => {}
        Ok(n) => {
            return Err(fail(
                ARM,
                format!("expected_eof_got_data:n={}:bytes={}", n, show(&buf[..n])),
            ))
        }
        Err(Error::Os(Errno::EAGAIN)) => {
            return Err(fail(
                ARM,
                "leaked_refcount_across_exec:post_parent_close_read=EAGAIN_not_EOF".to_string(),
            ))
        }
        Err(e) => return Err(fail(ARM, format!("expected_eof_got_error:{}", e))),
    }

    let _ = io::close(master);
    pass(ARM, "cloexec_survived_fork=1:eof_after_parent_close=1");
    Ok(())
}

/// All 14 arms run on both architectures. cloexec_exec (arm 14) was
/// excluded on x86 first pending #721 (exec() ENOSYS in the zero-feature
/// production build), then -- once #721 closed and re-admission surfaced a
/// second, distinct blocker -- pending #745 (x86 fork() unconditionally
/// refused in that same profile; arm_cloexec_exec() calls process::fork()
/// before it ever execs). Both are now closed.
const ARM_COUNT: u32 = 14;

fn run() -> Result<u32, Failure> {
    let (master, slave_path) = arm_openpt()?;
    let slave = match arm_nonblock(master, &slave_path) {
        Ok(fd) => fd,
        Err(f) => {
            let _ = io::close(master);
            return Err(f);
        }
    };

    let result = (|| {
        arm_isatty(master, slave)?;
        arm_termios(slave)?;
        arm_canonical(master, slave)?;
        arm_icrnl(master, slave)?;
        arm_raw(master, slave)?;
        arm_echo(master, slave)?;
        arm_onlcr(master, slave)?;
        arm_winsize(master, slave)?;
        arm_pgrp(master, slave)?;
        arm_hangup(master, slave)?;
        arm_ctty()?;
        arm_cloexec_exec()?;
        Ok(())
    })();

    if let Err(f) = result {
        let _ = io::close(slave);
        let _ = io::close(master);
        return Err(f);
    }

    // arm_hangup already closed the slave.
    let _ = io::close(master);
    Ok(ARM_COUNT)
}

fn main() {
    // Arm 14 (cloexec_exec) execs a fresh copy of this program to observe
    // what the KERNEL does to an FD_CLOEXEC-marked fd during exec() --
    // close_cloexec() runs before main() is ever reached. This marker mode
    // must do nothing else: touching any TTY state here would test what the
    // child does, not what the kernel already did.
    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 && args[1] == "--cloexec-child" {
        process::exit(0);
    }

    match run() {
        Ok(arms) => {
            emit(&format!("[TTY_ORACLE:COMPLETE:pass={}:fail=0]", arms));
            process::exit(0);
        }
        Err(f) => {
            emit(&format!("[TTY_ORACLE:FAIL:{}:{}]", f.arm, f.detail));
            emit("[TTY_ORACLE:COMPLETE:pass=0:fail=1]");
            process::exit(1);
        }
    }
}
