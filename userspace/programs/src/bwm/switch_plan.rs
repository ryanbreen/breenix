//! Geometry to apply only after SET_MODE succeeds; refusal is an identity plan.
use super::placement::{fit, Frame, Layout, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: usize,
    pub height: usize,
    pub scale: usize,
}

pub struct WindowPlan {
    pub frame: Frame,
    pub resize: Option<[u32; 2]>,
}

pub struct SwitchPlan {
    pub mode: Mode,
    pub layout: Layout,
    pub windows: Vec<WindowPlan>,
    pub refusal: Option<i64>,
}

pub fn switch_plan(old: Mode, new: Result<Mode, i64>, windows: &[Frame]) -> SwitchPlan {
    let (mode, refusal) = match new {
        Ok(mode) => (mode, None),
        Err(errno) => (old, Some(errno)),
    };
    let layout = Layout::for_mode(mode.width, mode.height, mode.scale);
    let windows = windows.iter().map(|&frame| {
        if refusal.is_some() { return WindowPlan { frame, resize: None }; }
        let ratio = |size: usize| size.saturating_mul(mode.scale) / old.scale;
        let content = [frame.rect.width.saturating_sub(frame.decoration[0]),
                       frame.rect.height.saturating_sub(frame.decoration[1])];
        let decoration = frame.decoration.map(ratio);
        // Window buffers have the same 4096-pixel limit as interactive resizing.
        let scaled = content.map(|size| ratio(size).clamp(1, 4096));
        let position = |value: i32| (i64::from(value) * mode.scale as i64 / old.scale as i64) as i32;
        let frame = fit(Frame {
            rect: Rect { x: position(frame.rect.x), y: position(frame.rect.y),
                width: scaled[0] + decoration[0], height: scaled[1] + decoration[1] },
            decoration,
        }, layout.work, [100 * mode.scale, 80 * mode.scale]);
        let size = [(frame.rect.width - decoration[0]) as u32,
                    (frame.rect.height - decoration[1]) as u32];
        // A mode change also tells unchanged-size clients to redraw for the display.
        WindowPlan { frame, resize: (mode != old || size != content.map(|n| n as u32)).then_some(size) }
    }).collect();
    SwitchPlan { mode, layout, windows, refusal }
}
