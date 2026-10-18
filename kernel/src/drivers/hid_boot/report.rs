//! HID boot keyboard reports to key events.
//!
//! Source authority: HID 1.11 Appendix B.1 (the 8-byte input report and the
//! 1-byte LED output report), Appendix C and F.3-F.5 (array semantics, edges
//! found by comparing consecutive reports, ErrorRollOver, host-side typematic
//! and LED synchronization), and the HID Usage Tables' Keyboard/Keypad page
//! (0x07) for usage IDs. Key codes are the published `input-event-codes.h`
//! values the input core takes. No other keyboard implementation was
//! consulted. The module is pure and allocation-free:
//! `rustc --edition=2021 --test report.rs` runs its tests.

pub const REPORT_LEN: usize = 8;

// Byte 0 of the input report: usages E0-E7, one bit each (B.1).
pub const LEFT_CTRL: u8 = 1 << 0;
pub const LEFT_SHIFT: u8 = 1 << 1;
pub const LEFT_ALT: u8 = 1 << 2;
pub const RIGHT_CTRL: u8 = 1 << 4;
pub const RIGHT_SHIFT: u8 = 1 << 5;
pub const RIGHT_ALT: u8 = 1 << 6;

/// The key codes of byte 0's bits: Ctrl, Shift, Alt and GUI, left then right.
const MODIFIER_CODES: [u16; 8] = [29, 42, 56, 125, 97, 54, 100, 126];

/// An event's value, as the input core takes it.
pub const RELEASE: i32 = 0;
pub const PRESS: i32 = 1;
pub const REPEAT: i32 = 2;

/// Delay before the first repeat and the repeat period (about 30 per second).
/// The keyboard has no typematic of its own (F.4); these are the host's.
pub const REPEAT_DELAY_NS: u64 = 500_000_000;
pub const REPEAT_PERIOD_NS: u64 = 33_333_333;

#[derive(Debug, PartialEq, Eq)]
pub enum ReportError {
    /// A boot report is exactly eight bytes (B.1).
    Length(usize),
}

/// What one report did, for the driver's markers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// A modifier or a key was down in this report.
    pub nonempty: bool,
    /// The keyboard reported ErrorRollOver, POSTFail or ErrorUndefined in its
    /// array; the previous key state stands (C, F.3).
    pub phantom: bool,
}

struct Repeat {
    code: u16,
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

    /// Decode one interrupt-IN report received at `now` (monotonic ns) into
    /// key events: `emit(code, value)`. Releases come before presses, and
    /// modifiers before keys, so a key pressed with its modifier carries it.
    pub fn report(&mut self, report: &[u8], now: u64, emit: &mut dyn FnMut(u16, i32)) -> Result<Summary, ReportError> {
        if report.len() != REPORT_LEN {
            return Err(ReportError::Length(report.len()));
        }
        let modifiers = report[0];
        // Byte 1 is reserved for OEM use and disregarded (B.1, F.4).
        let keys: [u8; 6] = [report[2], report[3], report[4], report[5], report[6], report[7]];
        let summary = Summary {
            nonempty: modifiers != 0 || keys.iter().any(|&k| k != 0),
            // The modifier byte stands in a phantom report; the array does not.
            phantom: keys.iter().any(|&k| (1..=3).contains(&k)),
        };
        let released = self.modifiers & !modifiers;
        let pressed = modifiers & !self.modifiers;
        self.modifiers = modifiers;
        emit_modifiers(released, RELEASE, emit);
        if !summary.phantom {
            // Order within a report has no meaning (C): releases, then presses.
            for &usage in self.keys.iter().filter(|&&k| k != 0 && !keys.contains(&k)) {
                if let Some(code) = usage_code(usage) {
                    emit(code, RELEASE);
                    if self.repeat.as_ref().is_some_and(|r| r.code == code) {
                        self.repeat = None;
                    }
                }
            }
        }
        emit_modifiers(pressed, PRESS, emit);
        if summary.phantom {
            return Ok(summary);
        }
        for (i, &usage) in keys.iter().enumerate() {
            if usage == 0 || self.keys.contains(&usage) || keys[..i].contains(&usage) {
                continue;
            }
            let Some(code) = usage_code(usage) else {
                continue;
            };
            emit(code, PRESS);
            // The lock keys toggle once per press and do not repeat.
            if !matches!(usage, 0x39 | 0x47 | 0x53) {
                self.repeat = Some(Repeat { code, due: now.saturating_add(REPEAT_DELAY_NS) });
            }
        }
        self.keys = keys;
        Ok(summary)
    }

    /// Repeat the held key when its time has come. A late call sends one
    /// repeat, not a burst, and schedules the next a period from now.
    pub fn tick(&mut self, now: u64, emit: &mut dyn FnMut(u16, i32)) {
        let Some(repeat) = self.repeat.as_mut() else {
            return;
        };
        if now < repeat.due {
            return;
        }
        emit(repeat.code, REPEAT);
        let next = repeat.due.saturating_add(REPEAT_PERIOD_NS);
        repeat.due = if next <= now { now.saturating_add(REPEAT_PERIOD_NS) } else { next };
    }

    /// The keyboard is gone: release each key and modifier it holds.
    pub fn release_all(&mut self, emit: &mut dyn FnMut(u16, i32)) {
        for &usage in self.keys.iter().filter(|&&k| k != 0) {
            if let Some(code) = usage_code(usage) {
                emit(code, RELEASE);
            }
        }
        emit_modifiers(self.modifiers, RELEASE, emit);
        *self = Self::new();
    }
}

fn emit_modifiers(bits: u8, value: i32, emit: &mut dyn FnMut(u16, i32)) {
    for (bit, &code) in MODIFIER_CODES.iter().enumerate() {
        if bits & 1 << bit != 0 {
            emit(code, value);
        }
    }
}

/// The key code of a Keyboard/Keypad usage. Letters follow the usage names
/// (04 is 'a'); 32, the non-US '#' key, is the backslash key's position on
/// those keyboards, and 64 the non-US '\' key. Print Screen, Pause,
/// Application, Power and the reserved and error usages have no key code here.
pub fn usage_code(usage: u8) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
    ];
    Some(match usage {
        0x04..=0x1D => LETTERS[(usage - 0x04) as usize],
        // '1' to '9' then '0': codes 2 to 11.
        0x1E..=0x27 => (usage - 0x1E) as u16 + 2,
        0x28 => 28,  // Enter
        0x29 => 1,   // Escape
        0x2A => 14,  // Backspace
        0x2B => 15,  // Tab
        0x2C => 57,  // Space
        0x2D => 12,  // - _
        0x2E => 13,  // = +
        0x2F => 26,  // [ {
        0x30 => 27,  // ] }
        0x31 | 0x32 => 43, // \ |, and the non-US # ~ key
        0x33 => 39,  // ; :
        0x34 => 40,  // ' "
        0x35 => 41,  // ` ~
        0x36 => 51,  // , <
        0x37 => 52,  // . >
        0x38 => 53,  // / ?
        0x39 => 58,  // Caps Lock
        // F1 to F10, then F11 and F12.
        0x3A..=0x43 => (usage - 0x3A) as u16 + 59,
        0x44 => 87,
        0x45 => 88,
        0x47 => 70,  // Scroll Lock
        0x49 => 110, // Insert
        0x4A => 102, // Home
        0x4B => 104, // Page Up
        0x4C => 111, // Delete
        0x4D => 107, // End
        0x4E => 109, // Page Down
        0x4F => 106, // Right
        0x50 => 105, // Left
        0x51 => 108, // Down
        0x52 => 103, // Up
        0x53 => 69,  // Num Lock
        0x54 => 98,  // Keypad /
        0x55 => 55,  // Keypad *
        0x56 => 74,  // Keypad -
        0x57 => 78,  // Keypad +
        0x58 => 96,  // Keypad Enter
        // Keypad 1-9, 0 and '.'.
        0x59..=0x63 => [79, 80, 81, 75, 76, 77, 71, 72, 73, 82, 83][(usage - 0x59) as usize],
        0x64 => 86,  // non-US \ |
        0x67 => 117, // Keypad =
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kb: &mut Keyboard, report: [u8; 8], now: u64) -> (Vec<(u16, i32)>, Summary) {
        let mut out = Vec::new();
        let summary = kb.report(&report, now, &mut |code, value| out.push((code, value))).unwrap();
        (out, summary)
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
        for len in [0, 7, 9, 64] {
            let r = vec![0u8; len];
            assert_eq!(kb.report(&r, 0, &mut |_, _| {}), Err(ReportError::Length(len)));
        }
    }

    #[test]
    fn presses_and_releases_are_edges_between_reports() {
        let mut kb = Keyboard::new();
        let (out, s) = run(&mut kb, key(0, &[0x04]), 0);
        assert_eq!(out, [(30, PRESS)]);
        assert!(s.nonempty);
        // Held across reports: no second press.
        assert!(run(&mut kb, key(0, &[0x04]), 1).0.is_empty());
        // A second key joins in either array position; only it is new.
        assert_eq!(run(&mut kb, key(0, &[0x05, 0x04]), 2).0, [(48, PRESS)]);
        // Release a, keep b; then press a again.
        assert_eq!(run(&mut kb, key(0, &[0x05]), 3).0, [(30, RELEASE)]);
        assert_eq!(run(&mut kb, key(0, &[0x05, 0x04]), 4).0, [(30, PRESS)]);
        let (out, s) = run(&mut kb, key(0, &[]), 5);
        assert_eq!(out, [(48, RELEASE), (30, RELEASE)]);
        assert!(!s.nonempty);
        // A repeated usage within one report is one press.
        assert_eq!(run(&mut kb, key(0, &[0x06, 0x06]), 6).0, [(46, PRESS)]);
    }

    #[test]
    fn usages_take_the_key_codes_of_their_names() {
        // a to z in usage order: the codes of the US keys with those legends.
        let letters: Vec<u16> = (0x04..=0x1D).filter_map(usage_code).collect();
        assert_eq!(letters, [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44]);
        let digits: Vec<u16> = (0x1E..=0x27).filter_map(usage_code).collect();
        assert_eq!(digits, (2..=11).collect::<Vec<u16>>());
        let function: Vec<u16> = (0x3A..=0x45).filter_map(usage_code).collect();
        assert_eq!(function, [59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88]);
        let navigation: Vec<u16> = (0x49..=0x52).filter_map(usage_code).collect();
        assert_eq!(navigation, [110, 102, 104, 111, 107, 109, 106, 105, 108, 103]);
        let keypad: Vec<u16> = (0x53..=0x63).filter_map(usage_code).collect();
        assert_eq!(keypad, [69, 98, 55, 74, 78, 96, 79, 80, 81, 75, 76, 77, 71, 72, 73, 82, 83]);
        assert_eq!([0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x39, 0x47, 0x64].map(|u| usage_code(u).unwrap()), [28, 1, 14, 15, 57, 58, 70, 86]);
        // No key code: the reserved and error usages, Print Screen, Pause, Application, Power.
        for usage in [0x00, 0x01, 0x02, 0x03, 0x46, 0x48, 0x65, 0x66, 0x68, 0xE0] {
            assert_eq!(usage_code(usage), None, "{usage:#x}");
        }
    }

    #[test]
    fn modifier_bits_are_keys_pressed_and_released_before_the_array() {
        let mut kb = Keyboard::new();
        // Right Shift and 'a' in one report: Shift first, so 'a' carries it.
        assert_eq!(run(&mut kb, key(RIGHT_SHIFT, &[0x04]), 0).0, [(54, PRESS), (30, PRESS)]);
        // Shift up and Ctrl down with 'c' in the next: the release, then the press, then 'c'.
        assert_eq!(run(&mut kb, key(LEFT_CTRL, &[0x04, 0x06]), 1).0, [(54, RELEASE), (29, PRESS), (46, PRESS)]);
        let (out, _) = run(&mut kb, key(LEFT_ALT | RIGHT_ALT | RIGHT_CTRL, &[]), 2);
        assert_eq!(out, [(29, RELEASE), (30, RELEASE), (46, RELEASE), (56, PRESS), (97, PRESS), (100, PRESS)]);
    }

    #[test]
    fn phantom_reports_keep_the_previous_keys() {
        let mut kb = Keyboard::new();
        run(&mut kb, key(0, &[0x04, 0x05]), 0);
        let (out, s) = run(&mut kb, key(LEFT_SHIFT, &[1, 1, 1, 1, 1, 1]), 1);
        // Only the modifier byte, which stays valid, changes anything.
        assert!(s.phantom);
        assert_eq!(out, [(42, PRESS)]);
        // Leaving the phantom state: a and b were down throughout, c is new.
        assert_eq!(run(&mut kb, key(LEFT_SHIFT, &[0x04, 0x05, 0x06]), 2).0, [(46, PRESS)]);
    }

    #[test]
    fn typematic_waits_500_ms_then_repeats_about_30_per_second() {
        let mut kb = Keyboard::new();
        let t0 = 1_000_000_000;
        assert_eq!(run(&mut kb, key(0, &[0x04]), t0).0, [(30, PRESS)]);
        assert_eq!(kb.repeat_due(), Some(t0 + REPEAT_DELAY_NS));
        let mut out = Vec::new();
        kb.tick(t0 + REPEAT_DELAY_NS - 1, &mut |c, v| out.push((c, v)));
        assert!(out.is_empty());
        // One second after the press: the first repeat at 500 ms, then one
        // per period, ticked at 5 ms steps as the worker's timer would.
        let mut t = t0 + REPEAT_DELAY_NS;
        while t <= t0 + 1_000_000_000 {
            kb.tick(t, &mut |c, v| out.push((c, v)));
            t += 5_000_000;
        }
        assert_eq!(out.len(), 1 + (500_000_000 / REPEAT_PERIOD_NS) as usize);
        assert!(out.iter().all(|&e| e == (30, REPEAT)));
        // A later key takes over the repeat; releasing it stops repeating
        // even though the first key is still down.
        run(&mut kb, key(0, &[0x04, 0x05]), t);
        out.clear();
        kb.tick(t + REPEAT_DELAY_NS, &mut |c, v| out.push((c, v)));
        assert_eq!(out, [(48, REPEAT)]);
        run(&mut kb, key(0, &[0x04]), t + 2 * REPEAT_DELAY_NS);
        assert_eq!(kb.repeat_due(), None);
        // A late tick sends one repeat, not a burst.
        run(&mut kb, key(0, &[]), 0);
        run(&mut kb, key(0, &[0x06]), 0);
        out.clear();
        kb.tick(60 * REPEAT_DELAY_NS, &mut |c, v| out.push((c, v)));
        assert_eq!(out, [(46, REPEAT)]);
        assert_eq!(kb.repeat_due(), Some(60 * REPEAT_DELAY_NS + REPEAT_PERIOD_NS));
    }

    #[test]
    fn lock_keys_press_once_and_do_not_repeat() {
        let mut kb = Keyboard::new();
        assert_eq!(run(&mut kb, key(0, &[0x39, 0x53, 0x47]), 0).0, [(58, PRESS), (69, PRESS), (70, PRESS)]);
        assert_eq!(kb.repeat_due(), None);
        kb.tick(10 * REPEAT_DELAY_NS, &mut |c, v| panic!("repeat of {c} {v}"));
    }

    #[test]
    fn a_detached_keyboard_releases_what_it_held() {
        let mut kb = Keyboard::new();
        run(&mut kb, key(LEFT_CTRL | RIGHT_ALT, &[0x3B]), 0);
        let mut out = Vec::new();
        kb.release_all(&mut |c, v| out.push((c, v)));
        assert_eq!(out, [(60, RELEASE), (29, RELEASE), (100, RELEASE)]);
        assert_eq!(kb.repeat_due(), None);
        // A fresh keyboard after reattach starts from a clean slate.
        assert_eq!(run(&mut kb, key(0, &[0x04]), 1).0, [(30, PRESS)]);
    }
}
