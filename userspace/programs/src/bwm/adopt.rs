//! Recover frame positions from the content geometry published by a previous compositor.
use super::{config::{BORDER_WIDTH, TITLE_BAR_HEIGHT}, session::WindowInfo};
use libbreenix::window::abi::win_flags;

pub(super) fn frame_origin(info: &WindowInfo, chromeless: bool, scale: usize) -> Option<(i32, i32)> {
    if info.flags & (win_flags::VISIBLE | win_flags::MINIMIZED) == 0 {
        return None;
    }
    let border = if chromeless { 0 } else { (BORDER_WIDTH * scale) as i32 };
    let title = if chromeless { 0 } else { (TITLE_BAR_HEIGHT * scale) as i32 };
    Some((info.x - border, info.y - title - border))
}
