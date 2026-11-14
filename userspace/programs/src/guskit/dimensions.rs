//! Logical document limits and initial client size.
pub const CANVAS_TOP: usize = 176;
// The largest client permitted by the window ABI, at scale 1, minus the palette.
pub const MAX_WIDTH: usize = libbreenix::window::abi::limits::MAX_DIMENSION as usize;
pub const MAX_HEIGHT: usize = MAX_WIDTH - CANVAS_TOP;

pub fn initial_window(screen_width: u32, screen_height: u32, scale: u32) -> (u32, u32) {
    let scale = scale.max(1);
    // bwm/config.rs: top taskbar 28, bottom appbar 36, title 32, borders 2 each.
    // CURRENT_MODE reports device dimensions; bwm derives its work area from it
    // (desk.rs::discover_windows). Leave a tenth of the client work area free.
    let width = (screen_width / scale).saturating_sub(4) * 9 / 10;
    let height = (screen_height / scale).saturating_sub(28 + 36 + 32 + 4) * 9 / 10;
    (width.max(800).min(MAX_WIDTH as u32 / scale) * scale,
     height.max(600).min(MAX_WIDTH as u32 / scale) * scale)
}
