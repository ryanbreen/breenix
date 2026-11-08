use super::*;

// ─── Hotkey System ──────────────────────────────────────────────────────────
// Configurable hotkey matching with multi-tap support. Hotkeys are loaded from
// /etc/hotkeys.conf at startup. Key chords use core event flags. Modifier edges
// feed the existing multi-tap matcher; UI-clock policy belongs to 21.4.

/// Modifier bits shared with the input core.
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
        let digest: String = libbreenix::crypto::sha256::sha256(&buf).iter().map(|b| format!("{b:02x}")).collect();

        let mut found_any = false;
        for line in buf.split(|&b| b == b'\n') {
            let line = trim(line);
            if line.is_empty() || line[0] == b'#' { continue; }
            if let Some(hotkey) = Self::parse_line(line) {
                self.bindings.push(hotkey);
                found_any = true;
            }
        }

        print!("[bwm] hotkeys: loaded {path} sha256={digest} bindings={}\n", self.bindings.len());
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

    /// Match a core key press using its modifier flags and physical press/repeat edge.
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

    /// Feed the existing modifier matcher a core edge; Super press edges are explicit.
    pub(super) fn update(&mut self, current_mods: u8, super_taps: u32, now_ns: u64) -> Option<HotkeyAction> {
        let prev = self.prev_modifiers;
        self.prev_modifiers = current_mods;

        // ── Super multi-tap detection driven by the input press edges ──
        // Each press-edge is one physical tap. If the press arrived while
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

                // Count this tap; continue a sequence only if the previous tap
                // was within the 400ms window, otherwise start a fresh sequence.
                if self.tap_count > 0
                    && super::timing::within(self.tap_release_ns, now_ns)
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
        // (Super is handled above by its press edges.) These modifiers are not affected
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

                    if self.tap_count > 0 && super::timing::within(self.tap_release_ns, now_ns) {
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

// ─── Window Discovery ───────────────────────────────────────────────────────
