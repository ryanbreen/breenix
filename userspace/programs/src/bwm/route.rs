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
pub(super) enum InputState {
    Normal,
    Escape,
    CsiOrSS3,
}

pub(super) enum InputEvent {
    /// A key press: ascii byte + the modifiers its own bytes carry (`modifier` bits).
    /// `repeat` is known only for keys sent with a modifier parameter (`ESC [ c ; m u`,
    /// console contract section 5.5): Some(true) for a repeat, Some(false) for a press.
    Key {
        ascii: u8,
        keycode: u16,
        modifiers: u16,
        repeat: Option<bool>,
    },
    /// F1..F5 for BWM-internal focus switching
    FunctionKey(u8),
}

impl InputParser {
    pub(super) fn new() -> Self {
        Self {
            escape_started_ms: 0,
            state: InputState::Normal,
            esc_buf: [0; 16],
            esc_len: 0,
        }
    }

    /// Resolve a bare Escape before admitting a later key as an Alt chord.
    pub(super) fn expire_escape(&mut self, now_ms: u64) -> Option<InputEvent> {
        if self.state == InputState::Escape
            && now_ms.saturating_sub(self.escape_started_ms) >= ESCAPE_TIMEOUT_MS
        {
            self.state = InputState::Normal;
            Some(Self::byte_to_key_event(0x1b))
        } else {
            None
        }
    }

    pub(super) fn feed(&mut self, byte: u8, now_ms: u64) -> Option<InputEvent> {
        match self.state {
            InputState::Normal => {
                if byte == 0x1b {
                    self.state = InputState::Escape;
                    self.escape_started_ms = now_ms;
                    self.esc_len = 0;
                    None
                } else {
                    Some(Self::byte_to_key_event(byte))
                }
            }
            InputState::Escape => {
                if byte == b'[' || byte == b'O' {
                    self.state = InputState::CsiOrSS3;
                    self.esc_buf[0] = byte;
                    self.esc_len = 1;
                    None
                } else {
                    // Alt sends ESC before the key's bytes.
                    self.state = InputState::Normal;
                    match Self::byte_to_key_event(byte) {
                        InputEvent::Key {
                            ascii,
                            keycode,
                            modifiers,
                            repeat,
                        } => Some(InputEvent::Key {
                            ascii,
                            keycode,
                            modifiers: modifiers | modifier::ALT as u16,
                            repeat,
                        }),
                        other => Some(other),
                    }
                }
            }
            InputState::CsiOrSS3 => {
                if self.esc_len < self.esc_buf.len() {
                    self.esc_buf[self.esc_len] = byte;
                    self.esc_len += 1;
                }
                if byte >= 0x40 && byte <= 0x7e {
                    self.state = InputState::Normal;
                    self.decode_escape()
                } else {
                    None
                }
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
        let ascii = match byte {
            0x7F => 8,
            _ => byte,
        };
        InputEvent::Key {
            ascii,
            keycode,
            modifiers,
            repeat: None,
        }
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
        if self.esc_len < 2 {
            return None;
        }
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
            let arrow = |keycode| {
                Some(InputEvent::Key {
                    ascii: 0,
                    keycode,
                    modifiers,
                    repeat,
                })
            };
            match final_byte {
                // A key with a modifier parameter: its byte, and its modifiers.
                b'u' if first <= 0x7F => {
                    return match Self::byte_to_key_event(first as u8) {
                        InputEvent::Key {
                            ascii,
                            keycode,
                            modifiers: own,
                            ..
                        } => Some(InputEvent::Key {
                            ascii,
                            keycode,
                            modifiers: own | modifiers,
                            repeat,
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
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
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
    #[cfg(not(target_arch = "aarch64"))]
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
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
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
            modifiers: modifier::SUPER,
            key: 0,
            taps: 2,
            action: HotkeyAction::Exec(b"/usr/bin/blauncher".to_vec()),
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::SUPER,
            key: b'\r',
            taps: 1,
            action: HotkeyAction::Exec(b"/usr/bin/bterm".to_vec()),
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT,
            key: b'q',
            taps: 1,
            action: HotkeyAction::Close,
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT,
            key: b'\t',
            taps: 1,
            action: HotkeyAction::FocusNext,
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT | modifier::SHIFT,
            key: b'\t',
            taps: 1,
            action: HotkeyAction::FocusPrev,
        });
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub(super) fn parse_line(line: &[u8]) -> Option<Hotkey> {
        // Split on '='
        let eq_pos = line.iter().position(|&b| b == b'=')?;
        let lhs = trim(&line[..eq_pos]);
        let rhs = trim(&line[eq_pos + 1..]);
        if lhs.is_empty() || rhs.is_empty() {
            return None;
        }

        // Parse left side: modifier+modifier+key
        let mut modifiers = 0u8;
        let mut key = 0u8;
        let mut taps = 1u8;

        // Count how many times each modifier appears (for multi-tap)
        let mut super_count = 0u8;

        let parts: Vec<&[u8]> = lhs.split(|&b| b == b'+').collect();
        for part in &parts {
            let part = trim(part);
            if part.is_empty() {
                continue;
            }
            match &part.to_ascii_lowercase()[..] {
                b"super" | b"cmd" | b"command" | b"win" => {
                    modifiers |= modifier::SUPER;
                    super_count += 1;
                }
                b"alt" => {
                    modifiers |= modifier::ALT;
                }
                b"ctrl" | b"control" => {
                    modifiers |= modifier::CTRL;
                }
                b"shift" => {
                    modifiers |= modifier::SHIFT;
                }
                b"return" | b"enter" => {
                    key = b'\r';
                }
                b"tab" => {
                    key = b'\t';
                }
                b"space" => {
                    key = b' ';
                }
                b"backspace" => {
                    key = 0x08;
                }
                b"escape" | b"esc" => {
                    key = 0x1b;
                }
                k if k.len() == 1 => {
                    key = k[0].to_ascii_lowercase();
                }
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
            if path.is_empty() {
                return None;
            }
            HotkeyAction::Exec(path.to_vec())
        } else {
            match &rhs.to_ascii_lowercase()[..] {
                b"close" => HotkeyAction::Close,
                b"focus_next" => HotkeyAction::FocusNext,
                b"focus_prev" => HotkeyAction::FocusPrev,
                _ => return None,
            }
        };

        Some(Hotkey {
            modifiers,
            key,
            taps,
            action,
        })
    }

    /// Match a stdin key press against the chord bindings, using the modifiers its own
    /// bytes carry. Bindings use ASCII (Return is CR), not the client event's USB HID
    /// keycode (Return is 0x28). `held` is the polled modifier state, used only to mark a
    /// modifier's hold as no clean tap. A matched chord is not delivered to a client:
    /// each press fires, a repeat is consumed, and a chord whose bytes cannot tell a press
    /// from a repeat (Alt's `ESC` prefix) fires once per cooldown.
    pub(super) fn key_pressed(
        &mut self,
        modifiers: u8,
        held: u8,
        ascii: u8,
        repeat: Option<bool>,
    ) -> Chord {
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
            binding.key != 0
                && binding.taps == 1
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
    /// captures each 0→1 SUPER transition the instant the HID report arrives,
    /// it recovers taps whose entire ~30ms high window fell between two bursty
    /// compositor-wait polls — which the level-based edge detection would miss.
    /// SUPER tap counting is driven directly by this latch so each physical
    /// press is counted exactly once (no double-count vs. release detection).
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
                if self.tap_count > 0 && now_ns.saturating_sub(self.tap_release_ns) < 400_000_000 {
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

                    if self.tap_count > 0
                        && now_ns.saturating_sub(self.tap_release_ns) < 400_000_000
                    {
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

#[cfg(not(target_arch = "aarch64"))]
pub(super) fn trim(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&b| b != b' ' && b != b'\t' && b != b'\r')
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&b| b != b' ' && b != b'\t' && b != b'\r')
        .map(|i| i + 1)
        .unwrap_or(start);
    &s[start..end]
}

pub(super) static INPUT_BACKLOG: std::sync::Mutex<input_backlog::Backlog<Input>> =
    std::sync::Mutex::new(input_backlog::Backlog::new());

pub(super) fn flush_input() {
    use input_backlog::Delivery;
    INPUT_BACKLOG.lock().unwrap().flush(
        |id, event| match session::send(id, *event) {
            Ok(1) => Delivery::Accepted,
            Ok(_) => Delivery::Full,
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => Delivery::Full,
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOENT)) => Delivery::Gone,
            Err(error) => {
                let _ = writeln!(
                    std::io::stderr(),
                    "bwm: window {id} input disabled: {error}"
                );
                Delivery::Failed
            }
        },
        |lost| Input::Overflow { lost },
    );
}

pub(super) fn send_to(windows: &[Window], win_idx: usize, input: Input) {
    if win_idx >= windows.len() || windows[win_idx].window_id == 0 {
        return;
    }
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
    send_to(
        windows,
        win_idx,
        if gained {
            Input::FocusGained
        } else {
            Input::FocusLost
        },
    );
}

// ─── Window Discovery ───────────────────────────────────────────────────────

impl State {
    pub(super) fn hotkey_state(&mut self) -> (u8, u32, Vec<HotkeyAction>) {
        // ── 0b. Poll modifier state and check hotkeys ──
        // Drain the kernel's latched Super press-edge count each frame (syscall 413's
        // key state) so a tap that completed between polls is fed into double-tap
        // detection.
        let keys = session::key_state().unwrap_or_default();
        let super_taps = keys.super_taps;
        let current_mods = keys.modifiers as u8;
        self.hotkey_mgr.cooldown = self.hotkey_mgr.cooldown.saturating_sub(1);
        // The chords recognized this frame, in the order their keys arrived.
        let hotkey_actions = Vec::new();

        (current_mods, super_taps, hotkey_actions)
    }
}

impl State {
    pub(super) fn keyboard(
        &mut self,
        stdin_ready: bool,
        current_mods: u8,
        super_taps: u32,
        mut hotkey_actions: Vec<HotkeyAction>,
    ) {
        // ── 2. Process keyboard input ──
        let now_ms = monotonic_ms().expect("bwm input requires the monotonic clock");
        let mut input_events: Vec<InputEvent> = self
            .input_parser
            .expire_escape(now_ms)
            .into_iter()
            .collect();
        if stdin_ready {
            if let Ok(n) = io::read(Fd::from_raw(0), &mut self.read_buf) {
                for i in 0..n {
                    if let Some(event) = self.input_parser.feed(self.read_buf[i], now_ms) {
                        input_events.push(event);
                    }
                }
            }
        }
        for event in input_events {
            match event {
                InputEvent::FunctionKey(k) if (k as usize) <= self.windows.len() => {
                    let new_focus = (k - 1) as usize;
                    if new_focus < self.windows.len() && new_focus != self.focused_win {
                        send_focus_event(&self.windows, self.focused_win, false);
                        self.focused_win = new_focus;
                        send_focus_event(&self.windows, self.focused_win, true);
                        compose_full_redraw(
                            &mut self.gpu,
                            &self.windows,
                            self.focused_win,
                            &self.clock_text,
                            &mut self.ui_font,
                        );
                        self.full_redraw = true;
                    }
                }
                InputEvent::Key {
                    ascii,
                    keycode,
                    modifiers,
                    repeat,
                } => {
                    match self
                        .hotkey_mgr
                        .key_pressed(modifiers as u8, current_mods, ascii, repeat)
                    {
                        Chord::Fire(action) => {
                            hotkey_actions.push(action);
                            continue;
                        }
                        Chord::Consumed => continue,
                        Chord::None => {}
                    }
                    if !self.windows.is_empty() {
                        send_to(
                            &self.windows,
                            self.focused_win,
                            Input::Key {
                                ascii,
                                keycode,
                                modifiers,
                                repeat: repeat == Some(true),
                            },
                        );
                    }
                }
                _ => {}
            }
        }

        hotkey_actions.extend(self.hotkey_mgr.update(current_mods, super_taps as u32));
        for action in &hotkey_actions {
            match action {
                HotkeyAction::FocusNext => {
                    if !self.windows.is_empty() {
                        send_focus_event(&self.windows, self.focused_win, false);
                        self.focused_win = next_visible_window(
                            &self.windows,
                            (self.focused_win + 1) % self.windows.len(),
                        );
                        send_focus_event(&self.windows, self.focused_win, true);
                        compose_full_redraw(
                            &mut self.gpu,
                            &self.windows,
                            self.focused_win,
                            &self.clock_text,
                            &mut self.ui_font,
                        );
                        self.full_redraw = true;
                    }
                }
                HotkeyAction::FocusPrev => {
                    if !self.windows.is_empty() {
                        send_focus_event(&self.windows, self.focused_win, false);
                        self.focused_win = if self.focused_win == 0 {
                            self.windows.len() - 1
                        } else {
                            self.focused_win - 1
                        };
                        self.focused_win = next_visible_window(&self.windows, self.focused_win);
                        send_focus_event(&self.windows, self.focused_win, true);
                        compose_full_redraw(
                            &mut self.gpu,
                            &self.windows,
                            self.focused_win,
                            &self.clock_text,
                            &mut self.ui_font,
                        );
                        self.full_redraw = true;
                    }
                }
                _ => execute_hotkey_action(action, &mut self.windows, self.focused_win),
            }
        }
    }
}

impl State {
    pub(super) fn pointer(&mut self, damage: &mut Damage) {
        // ── 3. Process mouse input ──
        damage.mouse_moved_this_frame = false;
        if let Ok((mx, my, buttons, scroll_delta)) = session::mouse_state_with_scroll() {
            let new_mx = mx as i32;
            let new_my = my as i32;
            let mouse_moved = new_mx != self.mouse_x || new_my != self.mouse_y;
            damage.mouse_moved_this_frame = mouse_moved;

            if mouse_moved {
                self.mouse_x = new_mx;
                self.mouse_y = new_my;

                if self.dragging.is_some() || self.resizing.is_some() {
                    self.drag_resize(damage);
                } else if !self.windows.is_empty()
                    && self.focused_win < self.windows.len()
                    && !self.windows[self.focused_win].minimized
                    && self.windows[self.focused_win].hit_content(self.mouse_x, self.mouse_y)
                {
                    let local_x = self.mouse_x - self.windows[self.focused_win].content_x();
                    let local_y = self.mouse_y - self.windows[self.focused_win].content_y();
                    send_to(
                        &self.windows,
                        self.focused_win,
                        Input::Move {
                            x: local_x,
                            y: local_y,
                        },
                    );
                }

                // Update cursor shape based on resize edge hover
                if self.dragging.is_none() && self.resizing.is_none() {
                    let mut hover_shape = cursor_shape::ARROW;
                    for i in (0..self.windows.len()).rev() {
                        if self.windows[i].minimized {
                            continue;
                        }
                        if let Some(edge) =
                            self.windows[i].hit_resize_edge(self.mouse_x, self.mouse_y)
                        {
                            hover_shape = match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => cursor_shape::RESIZE_NS,
                                ResizeEdge::Left | ResizeEdge::Right => cursor_shape::RESIZE_EW,
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                    cursor_shape::RESIZE_NWSE
                                }
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                    cursor_shape::RESIZE_NESW
                                }
                            };
                            break;
                        }
                        if self.windows[i].hit_any(self.mouse_x, self.mouse_y) {
                            break; // Inside window but not on edge
                        }
                    }
                    damage.cursor_dirty |= self.active_cursor_shape != hover_shape;
                    self.active_cursor_shape = hover_shape;
                }
            }

            // Release: end drag or route release event.
            // Per-endpoint button tracking in the kernel prevents dual USB HID
            // endpoints from racing (one endpoint can't cancel the other's press).
            if (buttons & 1) == 0 && (self.prev_buttons & 1) != 0 {
                if let Some((win_idx, _, _, _, _, _, orig_w, orig_h)) = self.resizing.take() {
                    // Reset cursor shape back to arrow
                    damage.cursor_dirty |= self.active_cursor_shape != cursor_shape::ARROW;
                    self.active_cursor_shape = cursor_shape::ARROW;
                    let new_cw = self.windows[win_idx].content_width();
                    let new_ch = self.windows[win_idx].content_height();
                    let old_cw = orig_w.saturating_sub(px(BORDER_WIDTH) * 2);
                    let old_ch = orig_h.saturating_sub(px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2);
                    print!(
                        "[bwm] resize end: id={} old={}x{} new={}x{} frame={}x{}\n",
                        self.windows[win_idx].window_id,
                        old_cw,
                        old_ch,
                        new_cw,
                        new_ch,
                        self.windows[win_idx].width,
                        self.windows[win_idx].height
                    );
                    save_window_defaults(&self.windows[win_idx], &mut self.win_defaults);
                } else if self.dragging.is_some() {
                    let drag_idx = self.dragging.unwrap().0;
                    self.dragging = None;
                    save_window_defaults(&self.windows[drag_idx], &mut self.win_defaults);
                } else if !self.windows.is_empty()
                    && self.focused_win < self.windows.len()
                    && !self.windows[self.focused_win].minimized
                    && self.windows[self.focused_win].hit_content(self.mouse_x, self.mouse_y)
                {
                    let local_x = self.mouse_x - self.windows[self.focused_win].content_x();
                    let local_y = self.mouse_y - self.windows[self.focused_win].content_y();
                    send_to(
                        &self.windows,
                        self.focused_win,
                        Input::Button {
                            button: 1,
                            pressed: false,
                            x: local_x,
                            y: local_y,
                        },
                    );
                }
            }

            // Scroll wheel: forward to focused window content area
            if scroll_delta != 0
                && !self.windows.is_empty()
                && self.focused_win < self.windows.len()
                && !self.windows[self.focused_win].minimized
                && self.windows[self.focused_win].window_id != 0
            {
                let local_x = self.mouse_x - self.windows[self.focused_win].content_x();
                let local_y = self.mouse_y - self.windows[self.focused_win].content_y();
                send_to(
                    &self.windows,
                    self.focused_win,
                    Input::Scroll {
                        x: local_x,
                        y: local_y,
                        delta_y: scroll_delta,
                    },
                );
            }

            // Click: focus change + drag or route click to client
            let new_click = (buttons & 1) != 0 && (self.prev_buttons & 1) == 0;
            self.prev_buttons = buttons;

            if new_click {
                // Check taskbar area — ignore clicks
                if self.mouse_y < px(TASKBAR_HEIGHT) as i32 {
                    // Taskbar click — no action
                }
                // Check appbar area
                else if self.mouse_y >= (self.screen_h - px(APPBAR_HEIGHT)) as i32 {
                    if let Some(idx) = appbar_hit_test(
                        &self.windows,
                        self.screen_w,
                        self.screen_h,
                        self.mouse_x,
                        self.mouse_y,
                    ) {
                        if idx == self.focused_win && !self.windows[idx].minimized {
                            // Click focused window button → minimize
                            self.windows[idx].minimized = true;
                            send_focus_event(&self.windows, self.focused_win, false);
                            self.focused_win = next_visible_window(&self.windows, self.focused_win);
                            send_focus_event(&self.windows, self.focused_win, true);
                        } else {
                            // Click unfocused/minimized → restore and focus
                            self.windows[idx].minimized = false;
                            // Bring to top of z-order
                            if idx < self.windows.len() - 1 {
                                let win = self.windows.remove(idx);
                                self.windows.push(win);
                            }
                            let top = self.windows.len() - 1;
                            if top != self.focused_win {
                                send_focus_event(&self.windows, self.focused_win, false);
                                self.focused_win = top;
                                send_focus_event(&self.windows, self.focused_win, true);
                            } else {
                                self.focused_win = top;
                            }
                        }
                        update_kernel_z_order(&self.windows);
                        compose_full_redraw(
                            &mut self.gpu,
                            &self.windows,
                            self.focused_win,
                            &self.clock_text,
                            &mut self.ui_font,
                        );
                        self.full_redraw = true;
                    }
                }
                // Check windows (existing logic with chrome button additions)
                else if !self.windows.is_empty() {
                    let mut clicked_idx: Option<usize> = None;
                    let mut clicked_title = false;
                    let mut clicked_resize: Option<ResizeEdge> = None;
                    for i in (0..self.windows.len()).rev() {
                        if self.windows[i].minimized {
                            continue;
                        }
                        // Check resize edges first (they extend slightly outside)
                        if let Some(edge) =
                            self.windows[i].hit_resize_edge(self.mouse_x, self.mouse_y)
                        {
                            clicked_idx = Some(i);
                            clicked_resize = Some(edge);
                            break;
                        }
                        let ht = self.windows[i].hit_title(self.mouse_x, self.mouse_y);
                        let ha = self.windows[i].hit_any(self.mouse_x, self.mouse_y);
                        if ht || ha {
                            clicked_idx = Some(i);
                            clicked_title = ht;
                            break;
                        }
                    }
                    if let Some(ci) = clicked_idx {
                        let z_changed = ci < self.windows.len() - 1;
                        if z_changed {
                            let win = self.windows.remove(ci);
                            self.windows.push(win);
                            update_kernel_z_order(&self.windows);
                        }
                        let top = self.windows.len() - 1;
                        let focus_changed = top != self.focused_win;

                        if focus_changed {
                            send_focus_event(&self.windows, self.focused_win, false);
                            self.focused_win = top;
                            send_focus_event(&self.windows, self.focused_win, true);
                        } else {
                            self.focused_win = top;
                        }

                        if let Some(edge) = clicked_resize {
                            // Start resize — set cursor shape for the edge
                            let shape = match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => cursor_shape::RESIZE_NS,
                                ResizeEdge::Left | ResizeEdge::Right => cursor_shape::RESIZE_EW,
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                    cursor_shape::RESIZE_NWSE
                                }
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                    cursor_shape::RESIZE_NESW
                                }
                            };
                            damage.cursor_dirty |= self.active_cursor_shape != shape;
                            self.active_cursor_shape = shape;
                            self.windows[top].resize_right = matches!(
                                edge,
                                ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
                            )
                            .then_some(self.windows[top].x + self.windows[top].width as i32);
                            self.windows[top].resize_bottom = matches!(
                                edge,
                                ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
                            )
                            .then_some(self.windows[top].y + self.windows[top].height as i32);
                            self.resizing = Some((
                                top,
                                edge,
                                self.mouse_x,
                                self.mouse_y,
                                self.windows[top].x,
                                self.windows[top].y,
                                self.windows[top].width,
                                self.windows[top].height,
                            ));
                        } else if clicked_title {
                            // Check chrome buttons before starting drag
                            if self.windows[top].hit_close_button(self.mouse_x, self.mouse_y) {
                                send_to(&self.windows, top, Input::Close);
                                if self.windows[top].owner_pid > 0 {
                                    let _ = signal::kill(
                                        self.windows[top].owner_pid as i32,
                                        signal::SIGTERM,
                                    );
                                }
                            } else if self.windows[top]
                                .hit_minimize_button(self.mouse_x, self.mouse_y)
                            {
                                self.windows[top].minimized = true;
                                if self.focused_win == top {
                                    send_focus_event(&self.windows, self.focused_win, false);
                                    self.focused_win =
                                        next_visible_window(&self.windows, self.focused_win);
                                    send_focus_event(&self.windows, self.focused_win, true);
                                }
                                update_kernel_z_order(&self.windows);
                                compose_full_redraw(
                                    &mut self.gpu,
                                    &self.windows,
                                    self.focused_win,
                                    &self.clock_text,
                                    &mut self.ui_font,
                                );
                                self.full_redraw = true;
                            } else {
                                self.dragging = Some((
                                    top,
                                    self.mouse_x - self.windows[top].x,
                                    self.mouse_y - self.windows[top].y,
                                ));
                            }
                        } else if self.windows[top].hit_content(self.mouse_x, self.mouse_y) {
                            let local_x = self.mouse_x - self.windows[top].content_x();
                            let local_y = self.mouse_y - self.windows[top].content_y();
                            send_to(
                                &self.windows,
                                self.focused_win,
                                Input::Button {
                                    button: 1,
                                    pressed: true,
                                    x: local_x,
                                    y: local_y,
                                },
                            );
                        }

                        // Full redraw for z-order or focus change
                        if !self.full_redraw && (z_changed || focus_changed) {
                            compose_full_redraw(
                                &mut self.gpu,
                                &self.windows,
                                self.focused_win,
                                &self.clock_text,
                                &mut self.ui_font,
                            );
                            self.full_redraw = true;
                        }
                    }
                }
            }
        }
    }
}
