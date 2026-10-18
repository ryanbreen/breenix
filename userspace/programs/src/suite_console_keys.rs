//! The keyboard cases (V5): `keys`, measuring `docs/console/contract.md` section 9
//! (bindings) and section 5.5 (input bytes) with keys the launcher injects (`HOSTKEY`, V6).
//!
//! The host key is the input; the evidence is read back in the guest: the foreground
//! terminal through `VT_GETSTATE` and `VT_WAITACTIVE`, the switch's log line in
//! `/proc/kmsg`, a terminal's input through `FIONREAD` and `read` in raw mode, a view's
//! position through `VT_SCROLLVIEW`, and signals in the processes that receive them.

use super::*;

/// How long a case waits for a host key's effect.
const KEY_MS: u64 = 3000;
const TIOCSPGRP: u64 = 0x5410;

/// The launcher's path, for a failure that names it.
fn method() -> String {
    let declared = libbreenix::suite::hostkey_method().unwrap_or("");
    declared.split(';').next().filter(|path| !path.is_empty()).unwrap_or("none").to_string()
}

/// Inject `chords`; its sequence number.
fn keys(chords: &str) -> Verdict<u64> {
    libbreenix::suite::hostkeys(chords)
}

fn late(seq: u64) -> String {
    format!("host key seq={seq} not received within {KEY_MS} ms ({})", method())
}

/// Press `chord` and wait until terminal `n` is the foreground with its acquisition complete.
fn key_switch(chord: &str, n: usize) -> Verdict {
    let seq = keys(chord)?;
    require(within(KEY_MS, || foreground().is_ok_and(|fg| fg == n)), &late(seq))?;
    wait_active(n)
}

/// Terminals in raw mode for the case, restored and flushed when it ends.
struct Raw(Vec<(Term, Termios)>);

impl Raw {
    fn on(numbers: &[usize]) -> Verdict<Raw> {
        let mut raw = Raw(Vec::new());
        for &n in numbers {
            let term = Term::vt(n)?;
            let saved = term.raw_mode()?;
            raw.0.push((term, saved));
        }
        Ok(raw)
    }

    fn term(&self, n: usize) -> &Term {
        let name = format!("/dev/tty{n}");
        &self.0.iter().find(|(term, _)| term.name == name).expect("a raw terminal of the case").0
    }

    /// Wait for `want` on terminal `n`'s input, then require exactly it there.
    fn expect(&self, n: usize, seq: u64, want: &[u8]) -> Verdict {
        let term = self.term(n);
        require(within(KEY_MS, || term.available() >= want.len() as i32), &late(seq))?;
        sleep(50);
        let mut got = [0u8; 64];
        let count = io::read(term.fd, &mut got).map_err(|e| format!("read tty{n}: {e}"))?;
        require(&got[..count] == want, &format!("tty{n} received {:?}, expected {:?}", &got[..count], want))
    }

    /// Terminal `n` holds no input.
    fn quiet(&self, n: usize) -> Verdict {
        let pending = self.term(n).available();
        require(pending == 0, &format!("tty{n} received {pending} bytes of keyboard input"))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        for (term, saved) in &self.0 {
            let _ = term.ok("TCFLSH", TCFLSH, 0);
            let _ = term.set_termios(saved);
        }
    }
}

/// Switch with each chord in turn, requiring the terminal and the switch's log line.
fn switches(steps: &[(&str, usize)]) -> Verdict {
    for &(chord, n) in steps {
        let line = format!("[console] foreground tty{n} (key {chord})");
        let before = count(&kmsg()?, &line);
        key_switch(chord, n)?;
        require(within(KEY_MS, || kmsg().is_ok_and(|log| count(&log, &line) == before + 1)), &format!("no line {line}"))?;
    }
    Ok(())
}

pub fn keys_alt_fn() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let raw = Raw::on(&[1, 2, 3, 4, 5, 6])?;
        switches(&[("alt-f2", 2), ("alt-f3", 3), ("alt-f4", 4), ("alt-f5", 5), ("alt-f6", 6), ("alt-f1", 1)])?;
        require(shown()?.0 == 1, "the screen does not show tty1 after Alt+F1")?;
        (1..=6).try_for_each(|n| raw.quiet(n))
    })
}

pub fn keys_ctrl_alt_fn() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let raw = Raw::on(&[1, 2, 3, 4, 5, 6])?;
        switches(&[
            ("ctrl-alt-f6", 6),
            ("ctrl-alt-f5", 5),
            ("ctrl-alt-f4", 4),
            ("ctrl-alt-f3", 3),
            ("ctrl-alt-f2", 2),
            ("ctrl-alt-f1", 1),
        ])?;
        // Ctrl+Alt+F7 to F12 are reserved and have no effect; the key sent after them shows they were processed.
        let seq = keys("ctrl-alt-f8,x")?;
        raw.expect(1, seq, b"x")?;
        require(foreground()? == 1, "Ctrl+Alt+F8 switched")?;
        (2..=6).try_for_each(|n| raw.quiet(n))
    })
}

pub fn keys_ctrl_alt_fn_graphics() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        // A display owner in VT_AUTO holds tty1: the key is consumed and refused, and logged.
        let refused = "[console] switch to tty2 refused: tty1 display owned (key ctrl-alt-f2)";
        let before = count(&kmsg()?, refused);
        let (taken, done) = (Gate::new()?, Gate::new()?);
        let mut owner = Child::spawn(|| {
            libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
            libbreenix::graphics::take_over_display().map_err(|e| format!("take display: {e}"))?;
            taken.open();
            let waited = done.wait(KEY_MS * 2, "the refused key");
            libbreenix::graphics::give_back_display().map_err(|e| format!("give back: {e}"))?;
            waited
        })?;
        taken.wait(KEY_MS, "the display owner")?;
        let seq = keys("ctrl-alt-f2")?;
        let logged = within(KEY_MS, || kmsg().is_ok_and(|log| count(&log, refused) == before + 1));
        let fg = foreground()?;
        done.open();
        owner.finish(KEY_MS)?;
        require(logged, &format!("{}: no line {refused}", late(seq)))?;
        require(fg == 1, "Ctrl+Alt+F2 switched away from a VT_AUTO display owner")?;

        // A graphics terminal whose owner chose VT_PROCESS is released to the key.
        switch_to(3)?;
        let ready = Gate::new()?;
        let mut owner = Child::spawn(|| {
            let tty3 = Term::vt(3)?;
            own(&tty3)?;
            tty3.ok("KDSETMODE(KD_GRAPHICS)", KDSETMODE, KD_GRAPHICS)?;
            ready.open();
            require(signalled(&RELEASES, 1, KEY_MS * 2), "no release signal for the key")?;
            tty3.ok("VT_RELDISP(1)", VT_RELDISP, 1).map(drop)
        })?;
        ready.wait(KEY_MS, "the VT_PROCESS owner")?;
        switches(&[("ctrl-alt-f2", 2)])?;
        owner.finish(KEY_MS)
    })
}

pub fn keys_alt_fn_graphics() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        switch_to(3)?;
        let raw = Raw::on(&[3])?;
        let tty3 = Term::vt(3)?;
        tty3.ok("KDSETMODE(KD_GRAPHICS)", KDSETMODE, KD_GRAPHICS)?;
        let result = (|| {
            // ESC, then F2's bytes, and for F7 the same: input, not a switch (contract 5.5, 9.1).
            let seq = keys("alt-f2")?;
            raw.expect(3, seq, b"\x1b\x1bOQ")?;
            let seq = keys("alt-f7")?;
            raw.expect(3, seq, b"\x1b\x1b[18~")?;
            require(foreground()? == 3, "Alt+Fn switched away from a graphics terminal")
        })();
        tty3.ok("KDSETMODE(KD_TEXT)", KDSETMODE, KD_TEXT)?;
        result
    })
}

pub fn keys_alt_left_right_wrap() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let raw = Raw::on(&[1, 2, 6])?;
        switches(&[("alt-left", 6), ("alt-right", 1), ("alt-right", 2), ("alt-left", 1)])?;
        [1, 2, 6].iter().try_for_each(|&n| raw.quiet(n))
    })
}

pub fn keys_alt_f7_ignored() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let raw = Raw::on(&[1])?;
        let seq = keys("alt-f7,alt-f12,x")?;
        raw.expect(1, seq, b"x")?;
        require(foreground()? == 1, "Alt+F7 or Alt+F12 switched")
    })
}

pub fn keys_foreground_owns_input() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let raw = Raw::on(&[1, 2, 3])?;
        switch_to(3)?;
        let seq = keys("k")?;
        raw.expect(3, seq, b"k")?;
        raw.quiet(1)?;
        raw.quiet(2)?;
        switch_to(2)?;
        let seq = keys("m")?;
        raw.expect(2, seq, b"m")?;
        raw.quiet(1)?;
        raw.quiet(3)
    })
}

/// A process leading its own session on terminal `n`, its foreground group,
/// counting SIGINT; `ready` opens once it is set up, and `body` then decides.
fn session_on(n: usize, ready: &Gate, body: impl FnOnce() -> Verdict) -> Verdict<Child> {
    Child::spawn(|| {
        let pgrp = libbreenix::process::setsid().map_err(|e| format!("setsid: {e}"))?;
        let term = Term::vt(n)?;
        term.ok("TIOCSCTTY", TIOCSCTTY, 0)?;
        let group = pgrp.raw() as i32;
        term.ok("TIOCSPGRP", TIOCSPGRP, &group as *const i32 as u64)?;
        handle(signal::SIGINT, on_interrupt)?;
        ready.open();
        body()
    })
}

pub fn keys_ctrl_c_foreground_only() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let (ready2, ready3, done) = (Gate::new()?, Gate::new()?, Gate::new()?);
        let mut background = session_on(2, &ready2, || {
            done.wait(KEY_MS * 3, "the foreground's Ctrl+C")?;
            require(INTERRUPTS.load(Ordering::SeqCst) == 0, "Ctrl+C on tty3 signalled the session on tty2")
        })?;
        let mut foreground_session = session_on(3, &ready3, || {
            require(signalled(&INTERRUPTS, 1, KEY_MS * 2), "Ctrl+C did not signal tty3's foreground process group")
        })?;
        ready2.wait(KEY_MS, "the session on tty2")?;
        ready3.wait(KEY_MS, "the session on tty3")?;
        switch_to(3)?;
        let seq = keys("ctrl-c")?;
        let delivered = foreground_session.finish(KEY_MS * 2).map_err(|e| format!("{}: {e}", late(seq)));
        done.open();
        let quiet = background.finish(KEY_MS);
        delivered?;
        quiet
    })
}

pub fn keys_caps_per_vt() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let raw = Raw::on(&[1, 2])?;
        let seq = keys("capslock,a")?;
        raw.expect(1, seq, b"A")?;
        switch_to(2)?;
        let seq = keys("a")?;
        raw.expect(2, seq, b"a")?;
        switch_to(1)?;
        let seq = keys("a")?;
        raw.expect(1, seq, b"A")?;
        let seq = keys("capslock,a")?;
        raw.expect(1, seq, b"a")
    })
}

pub fn keys_shift_pageup() -> CaseResult {
    measured(|| {
        let _back = BackToTty1;
        let tty2 = Term::vt(2)?;
        let label = mark("shift-page");
        let rows = fill(&tty2, &label, 150)?;
        switch_to(2)?;
        let raw = Raw::on(&[2])?;
        let half = (rows / 2) as i64;
        // The live screen's first line is the one `rows - 1` before the end.
        let top = 150 - (rows - 1);
        for (chord, back) in [("shift-pageup", half), ("shift-pageup", 2 * half), ("shift-pagedown", half), ("shift-pagedown", 0)] {
            let seq = keys(chord)?;
            require(within(KEY_MS, || tty2.scroll(0).is_ok_and(|lines| lines == back)), &format!("{}: view not {back} lines back after {chord}", late(seq)))?;
            if back == half {
                view(2, &format!("{label}-{:03}", top - half as usize))?;
            }
        }
        view(2, &format!("{label}-149"))?;
        raw.quiet(2)
    })
}
