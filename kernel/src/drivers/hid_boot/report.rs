//! HID boot keyboard reports to console input.
//!
//! Source authority: HID 1.11 Appendix B.1 (the 8-byte input report and the
//! 1-byte LED output report), Appendix C and F.3-F.5 (array semantics, edges
//! found by comparing consecutive reports, ErrorRollOver, host-side typematic
//! and LED synchronization), and the HID Usage Tables' Keyboard/Keypad page
//! (0x07) for usage IDs; the US layout is the legend each usage names. No
//! other keyboard implementation was consulted. The module is pure and
//! allocation-free: `rustc --edition=2021 --test report.rs` runs its tests.

pub const REPORT_LEN: usize = 8;

// Byte 0 of the input report: usages E0-E7, one bit each (B.1).
pub const LEFT_CTRL: u8 = 1 << 0;
pub const LEFT_SHIFT: u8 = 1 << 1;
pub const LEFT_ALT: u8 = 1 << 2;
pub const RIGHT_CTRL: u8 = 1 << 4;
pub const RIGHT_SHIFT: u8 = 1 << 5;
pub const RIGHT_ALT: u8 = 1 << 6;
const CTRL: u8 = LEFT_CTRL | RIGHT_CTRL;
const SHIFT: u8 = LEFT_SHIFT | RIGHT_SHIFT;
const ALT: u8 = LEFT_ALT | RIGHT_ALT;

// The LED output report (B.1): bit 0 Num Lock, bit 1 Caps Lock, bit 2 Scroll Lock.
pub const LED_NUM: u8 = 1 << 0;
pub const LED_CAPS: u8 = 1 << 1;
pub const LED_SCROLL: u8 = 1 << 2;

/// Delay before the first repeat and the repeat period (about 30 per second).
/// The keyboard has no typematic of its own (F.4); these are the host's.
pub const REPEAT_DELAY_NS: u64 = 500_000_000;
pub const REPEAT_PERIOD_NS: u64 = 33_333_333;

const ESC: u8 = 0x1b;

/// Chords kept for the virtual-terminal layer; they send the TTY no bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chord {
    /// Alt+F1 .. Alt+F6, numbered 1-6.
    AltFunction(u8),
    AltLeft,
    AltRight,
}

/// The bytes one key event sends; the longest is Alt with a 4-byte sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bytes {
    len: u8,
    data: [u8; 6],
}
impl Bytes {
    fn of(bytes: &[u8]) -> Self {
        let mut data = [0; 6];
        data[..bytes.len()].copy_from_slice(bytes);
        Self {
            len: bytes.len() as u8,
            data,
        }
    }
    fn with_escape(self) -> Self {
        let mut data = [ESC; 6];
        data[1..=self.len as usize].copy_from_slice(self.as_slice());
        Self {
            len: self.len + 1,
            data,
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Bytes(Bytes),
    Chord(Chord),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReportError {
    /// A boot report is exactly eight bytes (B.1).
    Length(usize),
}

/// What one report did, for the driver's markers and LED requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// A modifier or a key was down in this report.
    pub nonempty: bool,
    /// The keyboard reported ErrorRollOver, POSTFail or ErrorUndefined in its
    /// array; the previous key state stands (C, F.3).
    pub phantom: bool,
    /// A lock key changed the LED state; the host sends it (F.4).
    pub leds_changed: bool,
}

struct Repeat {
    usage: u8,
    due: u64,
}

pub struct Keyboard {
    modifiers: u8,
    keys: [u8; 6],
    repeat: Option<Repeat>,
}

impl Keyboard {
    pub const fn new() -> Self {
        Self {
            modifiers: 0,
            keys: [0; 6],
            repeat: None,
        }
    }

    /// When the held key repeats next, if one does.
    pub fn repeat_due(&self) -> Option<u64> {
        self.repeat.as_ref().map(|r| r.due)
    }

    /// Decode one interrupt-IN report received at `now` (monotonic ns).
    /// `leds` belongs to the console, shared by the attached keyboard pipes.
    pub fn report(
        &mut self,
        report: &[u8],
        now: u64,
        leds: &mut u8,
        emit: &mut dyn FnMut(Output),
    ) -> Result<Summary, ReportError> {
        if report.len() != REPORT_LEN {
            return Err(ReportError::Length(report.len()));
        }
        let modifiers = report[0];
        // Byte 1 is reserved for OEM use and disregarded (B.1, F.4).
        let keys: [u8; 6] = [
            report[2], report[3], report[4], report[5], report[6], report[7],
        ];
        let mut summary = Summary {
            nonempty: modifiers != 0 || keys.iter().any(|&k| k != 0),
            ..Summary::default()
        };
        self.modifiers = modifiers;
        if keys.iter().any(|&k| (1..=3).contains(&k)) {
            summary.phantom = true;
            return Ok(summary);
        }
        // Order within a report has no meaning (C): releases, then presses.
        for &key in self.keys.iter().filter(|&&k| k != 0) {
            if !keys.contains(&key) && self.repeat.as_ref().is_some_and(|r| r.usage == key) {
                self.repeat = None;
            }
        }
        for (i, &key) in keys.iter().enumerate() {
            if key == 0 || self.keys.contains(&key) || keys[..i].contains(&key) {
                continue;
            }
            if let Some(led) = lock_led(key) {
                *leds ^= led;
                summary.leds_changed = true;
                continue;
            }
            match self.translate(key, *leds) {
                Some(output @ Output::Bytes(_)) => {
                    emit(output);
                    self.repeat = Some(Repeat {
                        usage: key,
                        due: now.saturating_add(REPEAT_DELAY_NS),
                    });
                }
                Some(output @ Output::Chord(_)) => emit(output),
                None => {}
            }
        }
        self.keys = keys;
        Ok(summary)
    }

    /// Repeat the held key when its time has come. A late call sends one
    /// repeat, not a burst, and schedules the next a period from now.
    pub fn tick(&mut self, now: u64, leds: u8, emit: &mut dyn FnMut(Output)) {
        let Some(repeat) = self.repeat.as_ref() else {
            return;
        };
        if now < repeat.due {
            return;
        }
        let (usage, due) = (repeat.usage, repeat.due);
        match self.translate(usage, leds) {
            Some(output @ Output::Bytes(_)) => {
                emit(output);
                let next = due.saturating_add(REPEAT_PERIOD_NS);
                let next = if next <= now {
                    now.saturating_add(REPEAT_PERIOD_NS)
                } else {
                    next
                };
                self.repeat = Some(Repeat { usage, due: next });
            }
            // A modifier pressed since made it a chord or a key without bytes: stop.
            _ => self.repeat = None,
        }
    }

    /// The key a usage sends under the current modifiers and lock state.
    fn translate(&self, usage: u8, leds: u8) -> Option<Output> {
        let shift = self.modifiers & SHIFT != 0;
        let ctrl = self.modifiers & CTRL != 0;
        let alt = self.modifiers & ALT != 0;
        let num = leds & LED_NUM != 0;
        let caps = leds & LED_CAPS != 0;
        // Keypad 1-9, 0 and '.' (59-63) are navigation keys with Num Lock off.
        let usage = match usage {
            0x59..=0x63 if !num => KEYPAD_NAVIGATION[(usage - 0x59) as usize]?,
            u => u,
        };
        let byte = |b: u8| Some(Bytes::of(&[b]));
        let bytes = match usage {
            0x04..=0x1d => {
                let lower = b'a' + (usage - 0x04);
                if ctrl {
                    byte(lower - b'a' + 1)
                } else if shift != caps {
                    byte(lower.to_ascii_uppercase())
                } else {
                    byte(lower)
                }
            }
            0x1e..=0x38 => {
                let (plain, shifted) = PRINTABLE[(usage - 0x1e) as usize];
                match (ctrl, plain) {
                    // Ctrl with @ [ \ ] ^ _ gives NUL and 1B-1F; with Space, NUL.
                    (true, b'2') | (true, b' ') => byte(0),
                    (true, b'[') => byte(0x1b),
                    (true, b'\\') => byte(0x1c),
                    (true, b']') => byte(0x1d),
                    (true, b'6') => byte(0x1e),
                    (true, b'-') => byte(0x1f),
                    _ => byte(if shift { shifted } else { plain }),
                }
            }
            0x3a..=0x3f if alt => return Some(Output::Chord(Chord::AltFunction(usage - 0x39))),
            0x4f if alt => return Some(Output::Chord(Chord::AltRight)),
            0x50 if alt => return Some(Output::Chord(Chord::AltLeft)),
            0x49 => Some(Bytes::of(b"\x1b[2~")),
            0x4a => Some(Bytes::of(b"\x1b[H")),
            0x4b => Some(Bytes::of(b"\x1b[5~")),
            0x4c => Some(Bytes::of(b"\x1b[3~")),
            0x4d => Some(Bytes::of(b"\x1b[F")),
            0x4e => Some(Bytes::of(b"\x1b[6~")),
            0x4f => Some(Bytes::of(b"\x1b[C")),
            0x50 => Some(Bytes::of(b"\x1b[D")),
            0x51 => Some(Bytes::of(b"\x1b[B")),
            0x52 => Some(Bytes::of(b"\x1b[A")),
            0x54 => byte(b'/'),
            0x55 => byte(b'*'),
            0x56 => byte(b'-'),
            0x57 => byte(b'+'),
            0x58 => byte(b'\r'),
            0x59..=0x63 => byte(b"1234567890."[(usage - 0x59) as usize]),
            0x64 => byte(if shift { b'|' } else { b'\\' }),
            0x67 => byte(b'='),
            _ => None,
        }?;
        // Alt sends ESC before the key's bytes.
        Some(Output::Bytes(if alt { bytes.with_escape() } else { bytes }))
    }
}

/// Usages 1E-38 without and with Shift. Return, Escape, Backspace (DEL),
/// Tab and Space are 28-2C.
const PRINTABLE: [(u8, u8); 27] = [
    (b'1', b'!'),
    (b'2', b'@'),
    (b'3', b'#'),
    (b'4', b'$'),
    (b'5', b'%'),
    (b'6', b'^'),
    (b'7', b'&'),
    (b'8', b'*'),
    (b'9', b'('),
    (b'0', b')'),
    (b'\r', b'\r'),
    (0x1b, 0x1b),
    (0x7f, 0x7f),
    (b'\t', b'\t'),
    (b' ', b' '),
    (b'-', b'_'),
    (b'=', b'+'),
    (b'[', b'{'),
    (b']', b'}'),
    (b'\\', b'|'),
    (b'#', b'~'),
    (b';', b':'),
    (b'\'', b'"'),
    (b'`', b'~'),
    (b',', b'<'),
    (b'.', b'>'),
    (b'/', b'?'),
];

/// Keypad 1 End, 2 Down, 3 PageDown, 4 Left, 5 (no key), 6 Right, 7 Home,
/// 8 Up, 9 PageUp, 0 Insert, '.' Delete: the usage names of 59-63.
const KEYPAD_NAVIGATION: [Option<u8>; 11] = [
    Some(0x4d),
    Some(0x51),
    Some(0x4e),
    Some(0x50),
    None,
    Some(0x4f),
    Some(0x4a),
    Some(0x52),
    Some(0x4b),
    Some(0x49),
    Some(0x4c),
];

/// Caps Lock (39), Num Lock (53) and Scroll Lock (47) toggle their LEDs.
fn lock_led(usage: u8) -> Option<u8> {
    match usage {
        0x39 => Some(LED_CAPS),
        0x53 => Some(LED_NUM),
        0x47 => Some(LED_SCROLL),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kb: &mut Keyboard, leds: &mut u8, report: [u8; 8], now: u64) -> (Vec<Output>, Summary) {
        let mut out = Vec::new();
        let summary = kb.report(&report, now, leds, &mut |o| out.push(o)).unwrap();
        (out, summary)
    }
    fn text(outputs: &[Output]) -> Vec<u8> {
        outputs
            .iter()
            .flat_map(|o| match o {
                Output::Bytes(b) => b.as_slice().to_vec(),
                Output::Chord(_) => panic!("unexpected chord {o:?}"),
            })
            .collect()
    }
    fn key(modifiers: u8, keys: &[u8]) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[0] = modifiers;
        r[1] = 0xa5; // reserved byte: ignored
        r[2..2 + keys.len()].copy_from_slice(keys);
        r
    }

    #[test]
    fn report_length_is_exactly_eight() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        for len in [0, 7, 9, 64] {
            let r = vec![0u8; len];
            assert_eq!(kb.report(&r, 0, &mut leds, &mut |_| {}), Err(ReportError::Length(len)));
        }
    }

    #[test]
    fn presses_and_releases_are_edges_between_reports() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        let (out, s) = run(&mut kb, &mut leds, key(0, &[0x04]), 0);
        assert_eq!(text(&out), b"a");
        assert!(s.nonempty);
        // Held across reports: no second press.
        assert!(run(&mut kb, &mut leds, key(0, &[0x04]), 1).0.is_empty());
        // A second key joins in either array position; only it is new.
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x05, 0x04]), 2).0), b"b");
        // Release a, keep b; then press a again: one new 'a'.
        assert!(run(&mut kb, &mut leds, key(0, &[0x05]), 3).0.is_empty());
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x05, 0x04]), 4).0), b"a");
        let (out, s) = run(&mut kb, &mut leds, key(0, &[]), 5);
        assert!(out.is_empty() && !s.nonempty);
        // A repeated usage within one report is one press.
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x06, 0x06]), 6).0), b"c");
    }

    #[test]
    fn us_layout_follows_the_usage_names() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        let mut typed = Vec::new();
        let unshifted: Vec<u8> = (0x04..=0x38).filter(|&u| u != 0x28 && u != 0x29).collect();
        for &u in &unshifted {
            typed.extend(text(&run(&mut kb, &mut leds, key(0, &[u]), 0).0));
            run(&mut kb, &mut leds, key(0, &[]), 0);
        }
        assert_eq!(
            typed,
            b"abcdefghijklmnopqrstuvwxyz1234567890\x7f\t -=[]\\#;'`,./".to_vec()
        );
        let mut typed = Vec::new();
        for &u in &unshifted {
            typed.extend(text(&run(&mut kb, &mut leds, key(RIGHT_SHIFT, &[u]), 0).0));
            run(&mut kb, &mut leds, key(0, &[]), 0);
        }
        assert_eq!(
            typed,
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZ!@#$%^&*()\x7f\t _+{}|~:\"~<>?".to_vec()
        );
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x28]), 0).0), b"\r");
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x29]), 0).0), b"\x1b");
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x64]), 0).0), b"\\");
    }

    #[test]
    fn ctrl_gives_control_codes_so_ctrl_c_is_etx() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        assert_eq!(text(&run(&mut kb, &mut leds, key(LEFT_CTRL, &[0x06]), 0).0), [0x03]);
        assert_eq!(text(&run(&mut kb, &mut leds, key(RIGHT_CTRL, &[0x07, 0x06]), 0).0), [0x04]);
        run(&mut kb, &mut leds, key(0, &[]), 0);
        for (usage, code) in [(0x04, 0x01), (0x1d, 0x1a), (0x2f, 0x1b), (0x31, 0x1c),
            (0x30, 0x1d), (0x23, 0x1e), (0x2d, 0x1f), (0x1f, 0x00), (0x2c, 0x00)] {
            assert_eq!(text(&run(&mut kb, &mut leds, key(LEFT_CTRL, &[usage]), 0).0), [code]);
            run(&mut kb, &mut leds, key(0, &[]), 0);
        }
        // Caps Lock does not change a control code.
        run(&mut kb, &mut leds, key(0, &[0x39]), 0);
        run(&mut kb, &mut leds, key(0, &[]), 0);
        assert_eq!(text(&run(&mut kb, &mut leds, key(LEFT_CTRL, &[0x06]), 0).0), [0x03]);
    }

    #[test]
    fn caps_lock_toggles_letters_and_the_led_on_press_edges_only() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        let (out, s) = run(&mut kb, &mut leds, key(0, &[0x39]), 0);
        assert!(out.is_empty() && s.leds_changed);
        assert_eq!(leds, LED_CAPS);
        // Holding it leaves the LEDs as they are; it does not repeat.
        let (_, s) = run(&mut kb, &mut leds, key(0, &[0x39]), 1);
        assert!(!s.leds_changed);
        kb.tick(10 * REPEAT_DELAY_NS, leds, &mut |o| panic!("{o:?}"));
        run(&mut kb, &mut leds, key(0, &[]), 2);
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x04]), 3).0), b"A");
        assert_eq!(text(&run(&mut kb, &mut leds, key(LEFT_SHIFT, &[0x05]), 4).0), b"b");
        // Caps Lock leaves digits alone.
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x1e]), 5).0), b"1");
        run(&mut kb, &mut leds, key(0, &[]), 6);
        let (_, s) = run(&mut kb, &mut leds, key(0, &[0x39]), 7);
        assert!(s.leds_changed);
        assert_eq!(leds, 0);
        // Num Lock and Scroll Lock have their own bits.
        run(&mut kb, &mut leds, key(0, &[]), 8);
        run(&mut kb, &mut leds, key(0, &[0x53, 0x47]), 9);
        assert_eq!(leds, LED_NUM | LED_SCROLL);
    }

    #[test]
    fn navigation_keys_send_vt100_sequences() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        for (usage, seq) in [
            (0x52, &b"\x1b[A"[..]), (0x51, b"\x1b[B"), (0x4f, b"\x1b[C"), (0x50, b"\x1b[D"),
            (0x4a, b"\x1b[H"), (0x4d, b"\x1b[F"), (0x4c, b"\x1b[3~"), (0x4b, b"\x1b[5~"),
            (0x4e, b"\x1b[6~"), (0x49, b"\x1b[2~"),
        ] {
            assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[usage]), 0).0), seq, "{usage:#x}");
            run(&mut kb, &mut leds, key(0, &[]), 0);
        }
        // Keypad: navigation with Num Lock off, digits with it on.
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x5c]), 0).0), b"\x1b[D");
        run(&mut kb, &mut leds, key(0, &[]), 0);
        assert!(run(&mut kb, &mut leds, key(0, &[0x5d]), 0).0.is_empty());
        run(&mut kb, &mut leds, key(0, &[0x53]), 0);
        run(&mut kb, &mut leds, key(0, &[]), 0);
        let mut typed = Vec::new();
        for u in 0x59..=0x63 {
            typed.extend(text(&run(&mut kb, &mut leds, key(0, &[u]), 0).0));
            run(&mut kb, &mut leds, key(0, &[]), 0);
        }
        assert_eq!(typed, b"1234567890.");
        // Alt sends ESC first.
        assert_eq!(text(&run(&mut kb, &mut leds, key(LEFT_ALT, &[0x04]), 0).0), b"\x1ba");
        assert_eq!(text(&run(&mut kb, &mut leds, key(LEFT_ALT, &[0x4c, 0x04]), 0).0), b"\x1b\x1b[3~");
    }

    #[test]
    fn alt_function_and_alt_arrow_chords_stay_distinct_events() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        let mut chords = Vec::new();
        for usage in 0x3a..=0x3f {
            let (out, _) = run(&mut kb, &mut leds, key(RIGHT_ALT, &[usage]), 0);
            chords.extend(out);
            run(&mut kb, &mut leds, key(RIGHT_ALT, &[]), 0);
        }
        for usage in [0x50, 0x4f] {
            chords.extend(run(&mut kb, &mut leds, key(LEFT_ALT, &[usage]), 0).0);
            run(&mut kb, &mut leds, key(0, &[]), 0);
        }
        let expected: Vec<Output> = (1..=6)
            .map(Chord::AltFunction)
            .chain([Chord::AltLeft, Chord::AltRight])
            .map(Output::Chord)
            .collect();
        assert_eq!(chords, expected);
        // F7 with Alt, and F1 without, send no output; chords do not repeat.
        assert!(run(&mut kb, &mut leds, key(LEFT_ALT, &[0x40]), 0).0.is_empty());
        run(&mut kb, &mut leds, key(0, &[]), 0);
        assert!(run(&mut kb, &mut leds, key(0, &[0x3a]), 0).0.is_empty());
        run(&mut kb, &mut leds, key(0, &[]), 0);
        assert_eq!(run(&mut kb, &mut leds, key(LEFT_ALT, &[0x3a]), 0).0.len(), 1);
        assert_eq!(kb.repeat_due(), None);
    }

    #[test]
    fn phantom_reports_keep_the_previous_keys() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        run(&mut kb, &mut leds, key(0, &[0x04, 0x05]), 0);
        let (out, s) = run(&mut kb, &mut leds, key(LEFT_SHIFT, &[1, 1, 1, 1, 1, 1]), 1);
        assert!(out.is_empty() && s.phantom);
        // Leaving the phantom state: a and b were down throughout, c is new
        // and takes the Shift the phantom report carried.
        assert_eq!(text(&run(&mut kb, &mut leds, key(LEFT_SHIFT, &[0x04, 0x05, 0x06]), 2).0), b"C");
    }

    #[test]
    fn typematic_waits_500_ms_then_repeats_about_30_per_second() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        let t0 = 1_000_000_000;
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x04]), t0).0), b"a");
        assert_eq!(kb.repeat_due(), Some(t0 + REPEAT_DELAY_NS));
        let mut out = Vec::new();
        kb.tick(t0 + REPEAT_DELAY_NS - 1, leds, &mut |o| out.push(o));
        assert!(out.is_empty());
        // One second after the press: the first repeat at 500 ms, then one
        // per period, ticked at 5 ms steps as the worker's timer would.
        let mut t = t0 + REPEAT_DELAY_NS;
        while t <= t0 + 1_000_000_000 {
            kb.tick(t, leds, &mut |o| out.push(o));
            t += 5_000_000;
        }
        assert_eq!(out.len(), 1 + (500_000_000 / REPEAT_PERIOD_NS) as usize);
        assert!(text(&out).iter().all(|&b| b == b'a'));
        // Shift pressed while held: repeats follow the modifiers.
        run(&mut kb, &mut leds, key(LEFT_SHIFT, &[0x04]), t);
        out.clear();
        kb.tick(t + REPEAT_PERIOD_NS, leds, &mut |o| out.push(o));
        assert_eq!(text(&out), b"A");
        // A later key takes over the repeat; releasing it stops repeating
        // even though the first key is still down.
        run(&mut kb, &mut leds, key(0, &[0x04, 0x05]), t + 2 * REPEAT_PERIOD_NS);
        out.clear();
        kb.tick(t + 2 * REPEAT_PERIOD_NS + REPEAT_DELAY_NS, leds, &mut |o| out.push(o));
        assert_eq!(text(&out), b"b");
        run(&mut kb, &mut leds, key(0, &[0x04]), t + 3 * REPEAT_DELAY_NS);
        assert_eq!(kb.repeat_due(), None);
        // A late tick sends one repeat, not a burst.
        run(&mut kb, &mut leds, key(0, &[]), 0);
        run(&mut kb, &mut leds, key(0, &[0x06]), 0);
        out.clear();
        kb.tick(60 * REPEAT_DELAY_NS, leds, &mut |o| out.push(o));
        assert_eq!(text(&out), b"c");
        assert_eq!(kb.repeat_due(), Some(60 * REPEAT_DELAY_NS + REPEAT_PERIOD_NS));
    }

    #[test]
    fn a_fresh_console_has_no_keys_modifiers_leds_or_repeat() {
        let mut kb = Keyboard::new();
        let mut leds = 0;
        run(&mut kb, &mut leds, key(LEFT_SHIFT, &[0x39]), 0);
        run(&mut kb, &mut leds, key(LEFT_SHIFT, &[0x04]), 0);
        // A fresh console and keyboard start from a clean slate.
        kb = Keyboard::new();
        leds = 0;
        assert_eq!((leds, kb.repeat_due()), (0, None));
        assert_eq!(text(&run(&mut kb, &mut leds, key(0, &[0x04]), 0).0), b"a");
    }
    #[test]
    fn keyboards_share_console_locks_including_repeat_and_reattach() {
        let mut first = Keyboard::new();
        let mut second = Keyboard::new();
        let mut leds = 0;
        run(&mut first, &mut leds, key(0, &[0x39, 0x53, 0x47]), 0);
        assert_eq!(leds, LED_CAPS | LED_NUM | LED_SCROLL);
        assert_eq!(text(&run(&mut second, &mut leds, key(0, &[0x04, 0x59]), 0).0), b"A1");
        // Each keyboard has its own press edges, but toggles the same locks.
        run(&mut second, &mut leds, key(0, &[0x39, 0x53, 0x47, 0x59]), 1);
        assert_eq!(leds, 0);
        let mut out = Vec::new();
        second.tick(REPEAT_DELAY_NS, leds, &mut |o| out.push(o));
        assert_eq!(text(&out), b"\x1b[F");
        // Reattaching a device resets its keys, not the console's locks.
        run(&mut first, &mut leds, key(0, &[]), 2);
        run(&mut first, &mut leds, key(0, &[0x39]), 3);
        second = Keyboard::new();
        assert_eq!(text(&run(&mut second, &mut leds, key(0, &[0x04]), 4).0), b"A");
        assert_eq!(leds, LED_CAPS);
    }

}
