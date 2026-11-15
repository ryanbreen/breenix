/// Place a decorated window inside the desktop area between the bars.
/// If it cannot fit, keep its top-left at the desktop origin on that axis.
pub fn cascade_position(desktop: [usize; 4], window: [usize; 2], scale: usize, index: usize) -> (i32, i32) {
    let axis = |origin: usize, extent: usize, size: usize, margin: usize| {
        let room = extent.saturating_sub(size);
        let margin = margin.min(room);
        (origin + margin + (index * 50 * scale) % (room - margin + 1)) as i32
    };
    (axis(desktop[0], desktop[2], window[0], 30 * scale),
     axis(desktop[1], desktop[3], window[1], 10 * scale))
}

/// A pixel rectangle, with a signed origin for windows that are off-screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // The mode-change integration will consume this layout API.
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
}

/// A window's outer rectangle and the pixels occupied by its decorations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // The mode-change integration will consume this layout API.
pub struct Frame {
    pub rect: Rect,
    pub decoration: [usize; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // The mode-change integration will consume this layout API.
pub struct Layout {
    pub taskbar: Rect,
    pub appbar: Rect,
    pub work: Rect,
}

#[allow(dead_code)] // The mode-change integration will consume this layout API.
impl Layout {
    pub fn for_mode(width: usize, height: usize, ui_scale: usize) -> Self {
        let top = super::config::TASKBAR_HEIGHT * ui_scale;
        let bottom = super::config::APPBAR_HEIGHT * ui_scale;
        Self {
            taskbar: Rect { x: 0, y: 0, width, height: top },
            appbar: Rect { x: 0, y: height.saturating_sub(bottom) as i32, width, height: bottom },
            work: Rect { x: 0, y: top as i32, width, height: height.saturating_sub(top + bottom) },
        }
    }
}

/// Fit the outer frame, preserving the content minimum if the work area is smaller.
/// In that impossible-to-fit case its top-left remains at the work-area origin.
#[allow(dead_code)] // The mode-change integration will consume this layout API.
pub fn fit(frame: Frame, work: Rect, min_content: [usize; 2]) -> Frame {
    fn axis(pos: i32, size: usize, origin: i32, extent: usize, minimum: usize) -> (i32, usize) {
        let size = size.min(extent.max(minimum));
        let last = i64::from(origin) + extent.saturating_sub(size) as i64;
        ((i64::from(pos).clamp(i64::from(origin), last)) as i32, size)
    }
    let (x, width) = axis(frame.rect.x, frame.rect.width, work.x, work.width,
                          min_content[0].saturating_add(frame.decoration[0]));
    let (y, height) = axis(frame.rect.y, frame.rect.height, work.y, work.height,
                           min_content[1].saturating_add(frame.decoration[1]));
    Frame { rect: Rect { x, y, width, height }, decoration: frame.decoration }
}
