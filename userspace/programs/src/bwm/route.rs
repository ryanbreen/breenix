use super::*;

pub(super) mod modifier {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const SUPER: u8 = 8;
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
    pub(super) tap_release_ns: Option<u64>,
    pub(super) super_combo_used: bool,
}

impl HotkeyManager {
    pub(super) fn new() -> Self {
        Self { bindings: Vec::new(), tap_release_ns: None, super_combo_used: false }
    }

    /// Load hotkey config from a file. Format:
    ///   modifier+modifier+key = action
    ///   Super+Super = exec blauncher
    ///   Alt+F4 = close
    ///   Super+Return = exec bterm
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

}

impl State {
    pub(super) fn close_dispatch(&mut self, top: usize) {
        send_to(&self.windows, top, Input::Close);
        if self.windows[top].owner_pid > 0 {
            let _ = signal::kill(
                self.windows[top].owner_pid as i32,
                signal::SIGTERM,
            );
        }
    }
}
