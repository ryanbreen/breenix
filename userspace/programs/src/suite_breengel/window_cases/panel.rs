//! The window cases of panel (contract section 23).

use crate::*;

pub(crate) const PANEL: &[Case] = &[
    case("handoff", "A display-taking child under probe takes the display, gives it back, and the panel is restored exactly", panel_handoff),
    case("between-cases", "In a GUI suite the panel is drawn only between cases and restored exactly after each", panel_between_cases),
    case("cursor-restore", "Moving the pointer over the panel leaves no cursor trail", panel_cursor_restore),
    case("cursor-trail-detected", "With the panel cursor's save and restore disabled by a test control, the trail is detected", panel_cursor_trail_detected),
    case("kernel-screens", "The kernel's console screen redraws exactly after the display is given back", panel_kernel_screens),
];

fn panel_handoff() -> CaseResult { needs_window("18, 20, 23", "the panel restored exactly after a display-taking child, by SCANOUT_READBACK") }
fn panel_between_cases() -> CaseResult { needs_window("23", "the panel drawn only between GUI cases and restored exactly") }
fn panel_cursor_restore() -> CaseResult { needs_window("18, 23", "no cursor trail over the panel, by SCANOUT_READBACK") }
fn panel_cursor_trail_detected() -> CaseResult { needs_window("21, 23", "the panel-cursor-no-restore control's trail detected") }
fn panel_kernel_screens() -> CaseResult { needs_window("23", "the kernel console screen redrawn exactly after giveback") }
