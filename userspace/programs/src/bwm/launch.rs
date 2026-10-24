use super::*;

pub(super) fn execute_hotkey_action(
    action: &HotkeyAction,
    windows: &mut Vec<Window>,
    focused_win: usize,
) {
    match action {
        HotkeyAction::Exec(path) => {
            // For blauncher, toggle: if already open, close it; otherwise spawn
            if path == b"/usr/bin/blauncher" {
                let launcher_idx = windows
                    .iter()
                    .position(|w| w.chromeless && w.title_len > 1 && w.title[0] == 1);
                if let Some(idx) = launcher_idx {
                    send_to(windows, idx, Input::Close);
                    return;
                }
            }
            // Spawn the program — build null-terminated path on stack
            let mut spawn_buf = [0u8; 256];
            let len = path.len().min(254);
            spawn_buf[..len].copy_from_slice(&path[..len]);
            spawn_buf[len] = 0;
            let _ = libbreenix::process::spawn(&spawn_buf[..len + 1]);
        }
        HotkeyAction::Close => {
            if !windows.is_empty() && focused_win < windows.len() {
                send_to(windows, focused_win, Input::Close);
            }
        }
        HotkeyAction::FocusNext | HotkeyAction::FocusPrev => {
            // Focus cycling is handled in the main loop since it needs
            // mutable access to focused_win and redraw state.
            // The main loop checks for these after calling update().
        }
    }
}
