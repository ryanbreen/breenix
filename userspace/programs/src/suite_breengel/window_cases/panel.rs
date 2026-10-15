//! The window cases of panel (contract section 23).

use crate::*;

pub(crate) const PANEL: &[Case] = &[
    case("handoff", "A display-taking child under probe takes the display, gives it back, and the panel is restored exactly", panel_handoff),
    case("between-cases", "In a GUI suite the panel is drawn only between cases and restored exactly after each", panel_between_cases),
    case("cursor-restore", "Moving the pointer over the panel leaves no cursor trail", panel_cursor_restore),
    case("cursor-trail-detected", "With the panel cursor's save and restore disabled by a test control, the trail is detected", panel_cursor_trail_detected),
    case("kernel-screens", "The kernel's console screen redraws exactly after the display is given back", panel_kernel_screens),
];

// ---------------------------------------------------------------------------------------
// panel: the suite panel around window cases (contract section 23)
// ---------------------------------------------------------------------------------------

use super::composition::{give_back, paint, screen, take_over};
use super::kit::*;

fn first_difference(a: &[u8], b: &[u8]) -> Option<(usize, usize)> {
    (0..1280 * 800).find(|&i| a[i * 4..i * 4 + 4] != b[i * 4..i * 4 + 4]).map(|i| (i % 1280, i / 1280))
}

fn panel_handoff() -> CaseResult {
    interface()?;
    let (before, _) = screen()?;
    let f = || {
        if take_over() < 0 {
            return 2;
        }
        if paint([0x70, 0x10, 0x90, 0]).is_err() {
            return 3;
        }
        if give_back() < 0 {
            return 4;
        }
        0
    };
    let ended = child_end(5_000, || f()).cite("23")?;
    ensure(matches!(ended, Ended::Exit(0)), "23", "the display-taking child did not take, paint and give back the display")?;
    let (after, _) = screen()?;
    match first_difference(&before, &after) {
        None => Ok(()),
        Some((x, y)) => at("23", format!("after the child gave the display back, the panel differs at ({x}, {y})")),
    }
}

fn panel_between_cases() -> CaseResult {
    interface()?;
    let (a, _) = screen()?;
    let _ = time::sleep_ms(300);
    let (b, _) = screen()?;
    match first_difference(&a, &b) {
        None => Ok(()),
        Some((x, y)) => at("23", format!("the screen changed at ({x}, {y}) while this case ran; the panel is drawn only between cases")),
    }
}

fn panel_cursor_restore() -> CaseResult {
    missing("a way for a case to move the panel's pointer, so its trail can be observed", "23")
}

fn panel_cursor_trail_detected() -> CaseResult {
    missing("a way for a case to move the panel's pointer with panel-cursor-no-restore armed", "21, 23")
}

fn panel_kernel_screens() -> CaseResult {
    interface()?;
    // This runner owns the display; giving it back shows the kernel's console screen.
    let gave = give_back();
    ensure(gave >= 0, "18", format!("give_back_display by the runner {}", outcome(gave)))?;
    let shown = screen();
    let f = || {
        if take_over() < 0 {
            return 2;
        }
        if paint([0x05, 0x06, 0x07, 0]).is_err() {
            return 3;
        }
        if give_back() < 0 {
            return 4;
        }
        0
    };
    let ended = child_end(5_000, || f());
    let again = screen();
    let retook = take_over();
    let (console, _) = shown?;
    let (redrawn, _) = again?;
    ensure(retook >= 0, "18", format!("the runner's take_over_display {}", outcome(retook)))?;
    ensure(matches!(ended.cite("23")?, Ended::Exit(0)), "23", "the display-taking child did not take, paint and give back the display")?;
    match first_difference(&console, &redrawn) {
        None => Ok(()),
        Some((x, y)) => at("23", format!("after a process took the display and gave it back, the kernel's console screen differs at ({x}, {y})")),
    }
}
