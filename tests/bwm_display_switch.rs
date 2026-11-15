//! Run with rustc --test; exercise the production, GPU-independent switch plan.
mod config {
    pub const TASKBAR_HEIGHT: usize = 28;
    pub const APPBAR_HEIGHT: usize = 36;
}
#[path = "../userspace/programs/src/bwm/placement.rs"]
pub mod placement;
#[path = "../userspace/programs/src/bwm/switch_plan.rs"]
mod switch;
use placement::{Frame, Rect};
use switch::{switch_plan, Mode};

fn frame(x: i32, y: i32, content: [usize; 2], decoration: [usize; 2]) -> Frame {
    Frame { rect: Rect { x, y, width: content[0] + decoration[0], height: content[1] + decoration[1] }, decoration }
}

#[test]
fn scale_changes_resize_content_and_chrome_and_fit_all_windows() {
    let old = Mode { width: 1280, height: 1024, scale: 1 };
    let windows = [frame(30, 38, [300, 200], [4, 36]), frame(1100, 900, [750, 550], [4, 36]),
                   frame(-500, -500, [100, 100], [0, 0])];
    let doubled = switch_plan(old, Ok(Mode { width: 2560, height: 2048, scale: 2 }), &windows);
    assert_eq!(doubled.windows[0].frame, frame(60, 76, [600, 400], [8, 72]));
    assert_eq!(doubled.windows[0].resize, Some([600, 400]));
    assert_eq!(doubled.windows[2].frame.decoration, [0, 0]);
    assert_eq!(doubled.windows[2].resize, Some([200, 200]));
    // R240 and virtio sizes, with moved, oversized and chromeless clients.
    for (width, height, scale) in [(1024, 768, 1), (1280, 1024, 1), (2560, 2048, 2), (1280, 800, 1), (2560, 1600, 2)] {
        let plan = switch_plan(old, Ok(Mode { width, height, scale }), &windows);
        let work = plan.layout.work;
        for window in &plan.windows {
            let r = window.frame.rect;
            assert!(r.x >= work.x && r.y >= work.y);
            assert!(r.x as usize + r.width <= work.width);
            assert!(r.y as usize + r.height <= work.y as usize + work.height);
        }
    }
    let back = switch_plan(doubled.mode, Ok(old), &[doubled.windows[0].frame]);
    assert_eq!(back.windows[0].frame, windows[0]);
    assert_eq!(back.windows[0].resize, Some([300, 200]));
}

#[test]
fn unchanged_sizes_send_no_resize_and_refusal_preserves_even_offscreen_windows() {
    let old = Mode { width: 1280, height: 800, scale: 1 };
    let windows = [frame(30, 38, [300, 200], [4, 36]), frame(1800, -500, [2000, 1500], [4, 36])];
    let fitted = switch_plan(old, Ok(old), &windows);
    assert_eq!(fitted.windows[0].frame, windows[0]);
    assert_eq!(fitted.windows[0].resize, None);
    assert_eq!(fitted.windows[1].resize, Some([1276, 700]));
    let refused = switch_plan(old, Err(22), &windows);
    assert_eq!(refused.mode, old);
    assert_eq!(refused.refusal, Some(22));
    assert_eq!(refused.layout, placement::Layout::for_mode(1280, 800, 1));
    for (planned, original) in refused.windows.iter().zip(windows) {
        assert_eq!(planned.frame, original);
        assert_eq!(planned.resize, None);
    }
}
