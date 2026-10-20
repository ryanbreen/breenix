//! Host tests for the placement used by bwm window discovery; run with rustc --test.
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
