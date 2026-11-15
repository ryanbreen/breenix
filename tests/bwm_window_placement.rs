//! Host tests for the placement used by bwm window discovery; run with rustc --test.
mod config {
    pub const TASKBAR_HEIGHT: usize = 28;
    pub const APPBAR_HEIGHT: usize = 36;
}
#[path = "../userspace/programs/src/bwm/placement.rs"]
mod placement;

#[test]
fn cascaded_terminals_stay_between_the_bars() {
    for (width, height, scale) in [(2560, 1600, 2), (1280, 800, 1)] {
        let top = 28 * scale;
        let bottom = height - 36 * scale;
        let window = [750 + 4 * scale, 550 + 36 * scale];
        for index in 0..32 {
            let (x, y) = placement::cascade_position(
                [0, top, width, bottom - top], window, scale, index,
            );
            assert!(x >= 0 && x as usize + window[0] <= width, "window {}: x={}", index, x);
            assert!(y as usize >= top && y as usize + window[1] <= bottom, "window {}: y={}", index, y);
        }
    }
}

#[test]
fn exact_fit_and_oversized_windows_keep_the_top_left_accessible() {
    for size in [[1280, 736], [1500, 900], [1270, 730]] {
        for index in 0..16 {
            let (x, y) = placement::cascade_position([0, 28, 1280, 736], size, 1, index);
            assert!(x >= 0 && y >= 28);
            if size[0] >= 1280 { assert_eq!(x, 0); }
            if size[1] >= 736 { assert_eq!(y, 28); }
        }
    }
}

#[test]
fn mode_changes_keep_decorated_windows_reachable() {
    use placement::{fit, Frame, Layout, Rect};
    // The captured R240 EDID catalogue and the virtio catalogue, respectively.
    for catalogue in [&[(1024, 768, 1), (1280, 1024, 1), (2560, 2048, 2)][..],
                      &[(1280, 800, 1), (2560, 1600, 2)][..]] {
        for &(old_w, old_h, old_scale) in catalogue {
            let old = Layout::for_mode(old_w, old_h, old_scale);
            for &(width, height, scale) in catalogue {
                let new = Layout::for_mode(width, height, scale);
                assert_eq!(new.taskbar, Rect { x: 0, y: 0, width, height: 28 * scale });
                assert_eq!(new.appbar, Rect { x: 0, y: (height - 36 * scale) as i32, width, height: 36 * scale });
                assert_eq!(new.work, Rect { x: 0, y: (28 * scale) as i32, width, height: height - 64 * scale });
                let decoration = [4 * old_scale, 36 * old_scale];
                let size = [750 + decoration[0], 550 + decoration[1]];
                let mut positions = Vec::new();
                for index in 0..32 {
                    positions.push(placement::cascade_position([0, old.work.y as usize, old.work.width, old.work.height], size, old_scale, index));
                }
                positions.extend([(old_w as i32 - size[0] as i32, (old_h - 36 * old_scale) as i32 - size[1] as i32),
                                  (-5000, -5000), (10000, 10000)]);
                for (x, y) in positions {
                    for dimensions in [size, [old_w + width, old_h + height]] {
                        let frame = Frame { rect: Rect { x, y, width: dimensions[0], height: dimensions[1] }, decoration };
                        let result = fit(frame, new.work, [100, 80]);
                        let r = result.rect;
                        assert!(r.x >= new.work.x && r.y >= new.work.y);
                        assert!(r.x as i64 + r.width as i64 <= new.work.x as i64 + new.work.width as i64);
                        assert!(r.y as i64 + r.height as i64 <= new.work.y as i64 + new.work.height as i64);
                        assert!(r.width >= 100 + decoration[0] && r.height >= 80 + decoration[1]);
                        assert_eq!(result.decoration, decoration);
                        assert_eq!(r.width, dimensions[0].min(new.work.width));
                        assert_eq!(r.height, dimensions[1].min(new.work.height));
                        if x >= new.work.x && y >= new.work.y
                            && x as i64 + dimensions[0] as i64 <= new.work.x as i64 + new.work.width as i64
                            && y as i64 + dimensions[1] as i64 <= new.work.y as i64 + new.work.height as i64 {
                            assert_eq!(result, frame);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn minimum_content_takes_priority_when_work_area_is_too_small() {
    use placement::{fit, Frame, Rect};
    let frame = Frame { rect: Rect { x: -100, y: -100, width: 500, height: 500 }, decoration: [4, 36] };
    let work = Rect { x: 10, y: 28, width: 80, height: 100 };
    assert_eq!(fit(frame, work, [100, 80]).rect, Rect { x: 10, y: 28, width: 104, height: 116 });
}

#[test]
fn a_fitting_window_is_not_enlarged_to_the_minimum() {
    use placement::{fit, Frame, Rect};
    let frame = Frame { rect: Rect { x: 20, y: 50, width: 50, height: 70 }, decoration: [4, 36] };
    let work = Rect { x: 0, y: 28, width: 1024, height: 704 };
    assert_eq!(fit(frame, work, [100, 80]), frame);
}
