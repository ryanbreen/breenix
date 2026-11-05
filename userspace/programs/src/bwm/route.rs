use super::*;

// ─── Input Parser ────────────────────────────────────────────────────────────
// Parses stdin bytes (keyboard input) into InputEvents that BWM can either
// handle internally (F-key focus switching) or route to the focused client
// window as window events.

pub(super) const ESCAPE_TIMEOUT_MS: u64 = 30;

pub(super) struct InputParser {
    pub(super) escape_started_ms: u64,
    pub(super) state: InputState,
    pub(super) esc_buf: [u8; 16],
    pub(super) esc_len: usize,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum InputState { Normal, Escape, CsiOrSS3 }

pub(super) enum InputEvent {
    /// A key press: ascii byte + the modifiers its own bytes carry (`modifier` bits).
    /// `repeat` is known only for keys sent with a modifier parameter (`ESC [ c ; m u`,
    /// console contract section 5.5): Some(true) for a repeat, Some(false) for a press.
    Key { ascii: u8, keycode: u16, modifiers: u16, repeat: Option<bool> },
    /// F1..F5 for BWM-internal focus switching
    FunctionKey(u8),
}

impl InputParser {
    pub(super) fn new() -> Self { Self { escape_started_ms: 0, state: InputState::Normal, esc_buf: [0; 16], esc_len: 0 } }

    /// Resolve a bare Escape before admitting a later key as an Alt chord.
    pub(super) fn expire_escape(&mut self, now_ms: u64) -> Option<InputEvent> {
        if self.state == InputState::Escape
            && now_ms.saturating_sub(self.escape_started_ms) >= ESCAPE_TIMEOUT_MS {
            self.state = InputState::Normal;
            Some(Self::byte_to_key_event(0x1b))
        } else {
            None
        }
    }

    pub(super) fn feed(&mut self, byte: u8, now_ms: u64) -> Option<InputEvent> {
        match self.state {
            InputState::Normal => {
                if byte == 0x1b { self.state = InputState::Escape; self.escape_started_ms = now_ms; self.esc_len = 0; None }
                else { Some(Self::byte_to_key_event(byte)) }
            }
            InputState::Escape => {
                if byte == b'[' || byte == b'O' {
                    self.state = InputState::CsiOrSS3; self.esc_buf[0] = byte; self.esc_len = 1; None
                } else {
                    // Alt sends ESC before the key's bytes.
                    self.state = InputState::Normal;
                    match Self::byte_to_key_event(byte) {
                        InputEvent::Key { ascii, keycode, modifiers, repeat } => Some(InputEvent::Key {
                            ascii, keycode, modifiers: modifiers | modifier::ALT as u16, repeat,
                        }),
                        other => Some(other),
                    }
                }
            }
            InputState::CsiOrSS3 => {
                if self.esc_len < self.esc_buf.len() { self.esc_buf[self.esc_len] = byte; self.esc_len += 1; }
                if byte >= 0x40 && byte <= 0x7e { self.state = InputState::Normal; self.decode_escape() }
                else { None }
            }
        }
    }

    /// Convert a raw stdin byte into a Key event with appropriate keycode/modifiers.
    pub(super) fn byte_to_key_event(byte: u8) -> InputEvent {
        let (keycode, modifiers) = match byte {
            // Enter (before Ctrl range since 0x0D falls in 0x01..=0x1A)
            0x0D => (0x28, 0),
            // Backspace (0x08 falls in Ctrl range; 0x7F does not)
            0x08 | 0x7F => (0x2A, 0),
            // Tab (0x09 falls in Ctrl range)
            0x09 => (0x2B, 0),
            // Ctrl+A through Ctrl+Z (0x01..0x1A), excluding Enter/Backspace/Tab matched above
            0x01..=0x1A => (byte as u16, modifier::CTRL as u16),
            // Printable ASCII + space
            _ => (byte as u16, 0),
        };
        let ascii = match byte { 0x7F => 8, _ => byte };
        InputEvent::Key { ascii, keycode, modifiers, repeat: None }
    }

    /// The parameters of a CSI sequence: the first, and the second's modifier value and
    /// event type (`m` or `m:e`). Absent values are 0.
    pub(super) fn params(&self) -> (u32, u32, u32) {
        let mut values = [0u32; 3];
        let mut index = 0;
        for &b in &self.esc_buf[1..self.esc_len - 1] {
            match b {
                b'0'..=b'9' if index < 3 => values[index] = values[index] * 10 + (b - b'0') as u32,
                b';' | b':' => index += 1,
                _ => {}
            }
        }
        (values[0], values[1], values[2])
    }

    pub(super) fn decode_escape(&self) -> Option<InputEvent> {
        if self.esc_len < 2 { return None; }
        let final_byte = self.esc_buf[self.esc_len - 1];
        if self.esc_buf[0] == b'[' {
            let (first, m, event) = self.params();
            // The modifier parameter is 1 plus Shift 1, Alt 2, Ctrl 4, Super 8.
            let m = m.saturating_sub(1);
            let modifiers = ((m & 1 != 0) as u8 * modifier::SHIFT
                | (m & 2 != 0) as u8 * modifier::ALT
                | (m & 4 != 0) as u8 * modifier::CTRL
                | (m & 8 != 0) as u8 * modifier::SUPER) as u16;
            let repeat = (self.esc_len > 3).then_some(event == 2);
            let arrow = |keycode| Some(InputEvent::Key { ascii: 0, keycode, modifiers, repeat });
            match final_byte {
                // A key with a modifier parameter: its byte, and its modifiers.
                b'u' if first <= 0x7F => {
                    return match Self::byte_to_key_event(first as u8) {
                        InputEvent::Key { ascii, keycode, modifiers: own, .. } => Some(InputEvent::Key {
                            ascii, keycode, modifiers: own | modifiers, repeat,
                        }),
                        other => Some(other),
                    };
                }
                // Arrow keys -> Key events with USB HID keycodes
                b'A' => return arrow(0x52), // Up
                b'B' => return arrow(0x51), // Down
                b'C' => return arrow(0x4F), // Right
                b'D' => return arrow(0x50), // Left
                // F1..F4 with a modifier parameter (`ESC [ 1 ; m P`).
                b'P' => return Some(InputEvent::FunctionKey(1)),
                b'Q' => return Some(InputEvent::FunctionKey(2)),
                b'R' => return Some(InputEvent::FunctionKey(3)),
                b'S' => return Some(InputEvent::FunctionKey(4)),
                b'~' => match first {
                    1 | 11 => return Some(InputEvent::FunctionKey(1)),
                    12 => return Some(InputEvent::FunctionKey(2)),
                    13 => return Some(InputEvent::FunctionKey(3)),
                    14 => return Some(InputEvent::FunctionKey(4)),
                    15 => return Some(InputEvent::FunctionKey(5)),
                    _ => {}
                },
                _ => {}
            }
        } else if self.esc_buf[0] == b'O' {
            match final_byte {
                b'P' => return Some(InputEvent::FunctionKey(1)),
                b'Q' => return Some(InputEvent::FunctionKey(2)),
                b'R' => return Some(InputEvent::FunctionKey(3)),
                b'S' => return Some(InputEvent::FunctionKey(4)),
                _ => {}
            }
        }
        None
    }
}

// ─── Hotkey System ──────────────────────────────────────────────────────────
// Configurable hotkey matching with multi-tap support. Hotkeys are loaded from
// /etc/hotkeys.conf at startup. A key combo is matched on the modifiers the key's
// own bytes carry; multi-tap sequences (e.g. Super+Super) use the kernel's Super
// tap latch and the modifier state BWM polls each frame.

/// Modifier bitmask constants (syscall 413's key state, breengel.md section 17.6)
pub(super) mod modifier {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8  = 2;
    pub const ALT: u8   = 4;
    pub const SUPER: u8 = 8;
}

/// What a key press is to the chord bindings.
pub(super) enum Chord {
    /// It completes a chord: run the action.
    Fire(HotkeyAction),
    /// It is a chord's key that does not act again (a repeat); no client gets it.
    Consumed,
    /// It is not a chord: the focused client gets it.
    None,
}

/// What action a hotkey triggers
#[derive(Clone)]
pub(super) enum HotkeyAction {
    /// Spawn a program by path
    Exec(Vec<u8>),
    /// Close the focused window
    Close,
    /// Cycle focus to the next window
    FocusNext,
    /// Cycle focus to the previous window
    FocusPrev,
}

/// A single hotkey binding
#[derive(Clone)]
pub(super) struct Hotkey {
    /// Required modifier bitmask (e.g. SUPER | ALT)
    pub(super) modifiers: u8,
    /// ASCII key (0 for modifier-only combos like Super+Super)
    pub(super) key: u8,
    /// Number of taps required (1 = normal press, 2 = double-tap)
    pub(super) taps: u8,
    /// What to do when triggered
    pub(super) action: HotkeyAction,
}

/// Hotkey matcher state machine
pub(super) struct HotkeyManager {
    pub(super) bindings: Vec<Hotkey>,
    /// Previous modifier state (for edge detection)
    pub(super) prev_modifiers: u8,
    /// For multi-tap detection: which modifier was last cleanly released
    pub(super) tap_modifier: u8,
    /// Timestamp (monotonic ns) of last clean modifier release
    pub(super) tap_release_ns: u64,
    /// Number of clean taps counted so far
    pub(super) tap_count: u8,
    /// Whether any non-modifier key was pressed during current modifier hold
    pub(super) combo_used: bool,
    /// Super chords must not inherit ordinary typing with Shift.
    pub(super) super_combo_used: bool,
    /// Cooldown frames remaining (prevents rapid re-trigger)
    pub(super) cooldown: u32,
}

impl HotkeyManager {
    pub(super) fn new() -> Self {
        Self {
            bindings: Vec::new(),
            prev_modifiers: 0,
            tap_modifier: 0,
            tap_release_ns: 0,
            tap_count: 0,
            combo_used: false,
            super_combo_used: false,
            cooldown: 0,
        }
    }

    /// Load hotkey config from a file. Format:
    ///   modifier+modifier+key = action
    ///   Super+Super = exec blauncher
    ///   Alt+F4 = close
    ///   Super+Return = exec bterm
    pub(super) fn load_config(&mut self, path: &str) {
        let fd = match fs::open(path, fs::O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                print!("[bwm] hotkeys: no config at {}, using defaults\n", path);
                self.load_defaults();
                return;
            }
        };
        let mut buf = vec![0u8; 4096];
        let n = match io::read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = io::close(fd);
                print!("[bwm] hotkeys: failed to read config, using defaults\n");
                self.load_defaults();
                return;
            }
        };
        let _ = io::close(fd);
        buf.truncate(n);

        let mut found_any = false;
        for line in buf.split(|&b| b == b'\n') {
            let line = trim(line);
            if line.is_empty() || line[0] == b'#' { continue; }
            if let Some(hotkey) = Self::parse_line(line) {
                self.bindings.push(hotkey);
                found_any = true;
            }
        }

        if !found_any {
            print!("[bwm] hotkeys: config empty, using defaults\n");
            self.load_defaults();
        }
    }

    pub(super) fn load_defaults(&mut self) {
        self.bindings.clear();
        self.bindings.push(Hotkey {
            modifiers: modifier::SUPER, key: 0, taps: 2,
            action: HotkeyAction::Exec(b"/usr/bin/blauncher".to_vec()),
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::SUPER, key: b'\r', taps: 1,
            action: HotkeyAction::Exec(b"/usr/bin/bterm".to_vec()),
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT, key: b'q', taps: 1,
            action: HotkeyAction::Close,
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT, key: b'\t', taps: 1,
            action: HotkeyAction::FocusNext,
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT | modifier::SHIFT, key: b'\t', taps: 1,
            action: HotkeyAction::FocusPrev,
        });
    }

    pub(super) fn parse_line(line: &[u8]) -> Option<Hotkey> {
        // Split on '='
        let eq_pos = line.iter().position(|&b| b == b'=')?;
        let lhs = trim(&line[..eq_pos]);
        let rhs = trim(&line[eq_pos + 1..]);
        if lhs.is_empty() || rhs.is_empty() { return None; }

        // Parse left side: modifier+modifier+key
        let mut modifiers = 0u8;
        let mut key = 0u8;
        let mut taps = 1u8;

        // Count how many times each modifier appears (for multi-tap)
        let mut super_count = 0u8;

        let parts: Vec<&[u8]> = lhs.split(|&b| b == b'+').collect();
        for part in &parts {
            let part = trim(part);
            if part.is_empty() { continue; }
            match &part.to_ascii_lowercase()[..] {
                b"super" | b"cmd" | b"command" | b"win" => {
                    modifiers |= modifier::SUPER;
                    super_count += 1;
                }
                b"alt" => { modifiers |= modifier::ALT; }
                b"ctrl" | b"control" => { modifiers |= modifier::CTRL; }
                b"shift" => { modifiers |= modifier::SHIFT; }
                b"return" | b"enter" => { key = b'\r'; }
                b"tab" => { key = b'\t'; }
                b"space" => { key = b' '; }
                b"backspace" => { key = 0x08; }
                b"escape" | b"esc" => { key = 0x1b; }
                k if k.len() == 1 => { key = k[0].to_ascii_lowercase(); }
                _ => {} // Unknown token, skip
            }
        }

        // Super+Super means double-tap Super (no key, modifier appears twice)
        if super_count >= 2 && key == 0 {
            taps = 2;
        }

        // Parse right side: action
        let action = if rhs.starts_with(b"exec ") || rhs.starts_with(b"exec\t") {
            let path = trim(&rhs[5..]);
            if path.is_empty() { return None; }
            HotkeyAction::Exec(path.to_vec())
        } else {
            match &rhs.to_ascii_lowercase()[..] {
                b"close" => HotkeyAction::Close,
                b"focus_next" => HotkeyAction::FocusNext,
                b"focus_prev" => HotkeyAction::FocusPrev,
                _ => return None,
            }
        };

        Some(Hotkey { modifiers, key, taps, action })
    }

    /// Match a stdin key press against the chord bindings, using the modifiers its own
    /// bytes carry. Bindings use ASCII (Return is CR), not the client event's USB HID
    /// keycode (Return is 0x28). `held` is the polled modifier state, used only to mark a
    /// modifier's hold as no clean tap. A matched chord is not delivered to a client:
    /// each press fires, a repeat is consumed, and a chord whose bytes cannot tell a press
    /// from a repeat (Alt's `ESC` prefix) fires once per cooldown.
    pub(super) fn key_pressed(&mut self, modifiers: u8, held: u8, ascii: u8, repeat: Option<bool>) -> Chord {
        if modifiers | held != 0 {
            self.combo_used = true;
        }
        if modifiers & modifier::SUPER != 0 {
            // The Super press of a chord is not a tap.
            self.super_combo_used = true;
            self.tap_count = 0;
            self.tap_release_ns = 0;
        }
        let Some(binding) = self.bindings.iter().find(|binding| {
            binding.key != 0 && binding.taps == 1
                && binding.key == ascii.to_ascii_lowercase()
                && (modifiers & binding.modifiers) == binding.modifiers
        }) else {
            return Chord::None;
        };
        match repeat {
            Some(true) => Chord::Consumed,
            Some(false) => Chord::Fire(binding.action.clone()),
            None if self.cooldown == 0 => {
                self.cooldown = 30;
                Chord::Fire(binding.action.clone())
            }
            None => Chord::Consumed,
        }
    }

    /// Called after draining this frame's keys, so a Super chord marks its tap
    /// dirty before modifier-only bindings are considered.
    ///
    /// `super_taps` is the count of Super press-edges latched in the kernel HID
    /// path since the previous frame (op=31, read-and-clear). Because the latch
    /// captures 0→1 SUPER transitions as the HID report arrives,
    /// it recovers taps whose entire ~30ms high window fell between two bursty
    /// compositor-wait polls — which the level-based edge detection would miss.
    /// SUPER tap counting uses this latch to avoid double-counting against release detection.
    pub(super) fn update(&mut self, current_mods: u8, super_taps: u32) -> Option<HotkeyAction> {
        let prev = self.prev_modifiers;
        self.prev_modifiers = current_mods;

        // ── Super multi-tap detection driven by the kernel press-edge latch ──
        // Each latched press-edge is one physical tap. If the press arrived while
        // a combo was in progress (a non-modifier key was held with Super), the
        // tap is treated as dirty and resets the sequence rather than counting.
        if super_taps > 0 {
            for _ in 0..super_taps {
                if self.super_combo_used {
                    // Combo in progress: this Super press is part of a combo, not
                    // a clean tap. Reset the tap sequence; do not fire the launcher.
                    self.tap_count = 0;
                    self.tap_release_ns = 0;
                    continue;
                }

                self.tap_modifier = modifier::SUPER;

                let now_ns = match libbreenix::time::now_monotonic() {
                    Ok(ts) => ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64,
                    Err(_) => 0,
                };

                // Count this tap; continue a sequence only if the previous tap
                // was within the 400ms window, otherwise start a fresh sequence.
                if self.tap_count > 0
                    && now_ns.saturating_sub(self.tap_release_ns) < 400_000_000
                {
                    self.tap_count += 1;
                } else {
                    self.tap_count = 1;
                }
                self.tap_release_ns = now_ns;

                // Fire the matching multi-tap binding (e.g. double-tap Super).
                if self.cooldown == 0 {
                    for binding in &self.bindings {
                        if binding.key == 0
                            && binding.modifiers == modifier::SUPER
                            && binding.taps == self.tap_count
                        {
                            self.cooldown = 30;
                            self.tap_count = 0;
                            self.tap_release_ns = 0;
                            return Some(binding.action.clone());
                        }
                    }
                }
            }
        }

        // Reset combo tracking when Super is released so the next clean
        // tap sequence isn't suppressed by a stale combo flag.
        let super_now = (current_mods & modifier::SUPER) != 0;
        if !super_now {
            self.super_combo_used = false;
        }

        // ── Multi-tap detection for ALT / CTRL / SHIFT via level edges ──
        // (Super is handled above by the latch.) These modifiers are not affected
        // by the launcher drop bug; keep their existing release-edge behavior.
        for &mod_bit in &[modifier::ALT, modifier::CTRL, modifier::SHIFT] {
            let was = (prev & mod_bit) != 0;
            let now = (current_mods & mod_bit) != 0;

            if now && !was {
                if mod_bit != self.tap_modifier {
                    self.tap_modifier = mod_bit;
                    self.tap_count = 0;
                }
                self.combo_used = false;
            } else if !now && was {
                if mod_bit == self.tap_modifier && !self.combo_used {
                    let now_ns = match libbreenix::time::now_monotonic() {
                        Ok(ts) => ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64,
                        Err(_) => 0,
                    };

                    if self.tap_count > 0 && now_ns.saturating_sub(self.tap_release_ns) < 400_000_000 {
                        self.tap_count += 1;
                    } else {
                        self.tap_count = 1;
                    }
                    self.tap_release_ns = now_ns;

                    if self.cooldown == 0 {
                        for binding in &self.bindings {
                            if binding.key == 0
                                && binding.modifiers == mod_bit
                                && binding.taps == self.tap_count
                            {
                                self.cooldown = 30;
                                self.tap_count = 0;
                                self.tap_release_ns = 0;
                                return Some(binding.action.clone());
                            }
                        }
                    }
                } else if mod_bit == self.tap_modifier {
                    self.tap_count = 0;
                    self.tap_release_ns = 0;
                }
            }
        }

        None
    }
}

pub(super) fn trim(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| b != b' ' && b != b'\t' && b != b'\r').unwrap_or(s.len());
    let end = s.iter().rposition(|&b| b != b' ' && b != b'\t' && b != b'\r').map(|i| i + 1).unwrap_or(start);
    &s[start..end]
}

// ─── Input Routing ──────────────────────────────────────────────────────────

pub(super) static INPUT_BACKLOG: std::sync::Mutex<input_backlog::Backlog<Input>> =
    std::sync::Mutex::new(input_backlog::Backlog::new());

pub(super) fn flush_input() {
    use input_backlog::Delivery;
    INPUT_BACKLOG.lock().unwrap().flush(|id, event| {
        match session::send(id, *event) {
            Ok(1) => Delivery::Accepted,
            Ok(_) => Delivery::Full,
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => Delivery::Full,
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOENT)) => Delivery::Gone,
            Err(error) => {
                let _ = writeln!(std::io::stderr(), "bwm: window {id} input disabled: {error}");
                Delivery::Failed
            }
        }
    }, |lost| Input::Overflow { lost });
}

pub(super) fn send_to(windows: &[Window], win_idx: usize, input: Input) {
    if win_idx >= windows.len() || windows[win_idx].window_id == 0 { return; }
    let id = windows[win_idx].window_id;
    let mut backlog = INPUT_BACKLOG.lock().unwrap();
    let admission = backlog.push(id, input, |old, new| {
        matches!((old, new), (Input::Move { .. }, Input::Move { .. }))
    });
    if admission == input_backlog::Admission::Disabled && matches!(input, Input::Close) {
        backlog.request_close(id, input);
    }
    drop(backlog);
    flush_input();
}

pub(super) fn send_focus_event(windows: &[Window], win_idx: usize, gained: bool) {
    send_to(windows, win_idx, if gained { Input::FocusGained } else { Input::FocusLost });
}

// ─── Window Discovery ───────────────────────────────────────────────────────
