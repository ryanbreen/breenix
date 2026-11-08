//! Pure pointer resolution; session code supplies the current window visibility.
use libbui::tree::{Flag, Node};

// Widen before forming half-open endpoints: tree rectangles carry signed i64s.
fn edges([x, y, w, h]: [i64; 4]) -> Option<[i128; 4]> {
    if w <= 0 || h <= 0 { return None; }
    Some([i128::from(x), i128::from(y), i128::from(x) + i128::from(w), i128::from(y) + i128::from(h)])
}
fn intersection(a: [i128; 4], b: [i128; 4]) -> Option<[i128; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// Resolve window-local bounds and clip against exposed rectangles in screen pixels.
/// Choose the largest intersection, breaking equal-area ties topmost then leftmost.
/// The caller confirms this screen point with the current scene's window hit test.
pub fn click_target(node: &Node, origin: (i32, i32), visible_region: &[[i64; 4]]) -> Result<(i32, i32), String> {
    let missing = || format!("input-target: {} has no visible area", node.key);
    if !node.flags.contains(&Flag::Visible) { return Err(missing()); }
    let bounds = edges(node.bounds).ok_or_else(missing)?;
    let clip = edges(node.clip).ok_or_else(missing)?;
    let mut local = intersection(bounds, clip).ok_or_else(missing)?;
    for i in [0, 2] { local[i] += i128::from(origin.0); }
    for i in [1, 3] { local[i] += i128::from(origin.1); }
    let best = visible_region.iter().filter_map(|&rect| intersection(local, edges(rect)?))
        .max_by_key(|r| ((r[2] - r[0]) * (r[3] - r[1]), -r[1], -r[0]));
    let r = best.ok_or_else(missing)?;
    let x = r[0] + (r[2] - r[0]) / 2;
    let y = r[1] + (r[3] - r[1]) / 2;
    let x = i32::try_from(x).map_err(|_| format!("input-target: {} x outside pointer range", node.key))?;
    let y = i32::try_from(y).map_err(|_| format!("input-target: {} y outside pointer range", node.key))?;
    Ok((x, y))
}

/// Home to (0,0), then reach the screen point with exact relative deltas.
/// bwm adds each delta without acceleration and clamps it to 0..dimension-1.
pub fn home_then_move(screen: (usize, usize), target: (i32, i32)) -> Result<[(i32, i32); 2], String> {
    let width = i32::try_from(screen.0).map_err(|_| "input-target: screen width outside pointer range")?;
    let height = i32::try_from(screen.1).map_err(|_| "input-target: screen height outside pointer range")?;
    if width <= 0 || height <= 0 || target.0 < 0 || target.1 < 0 || target.0 >= width || target.1 >= height {
        return Err(format!("input-target: point {target:?} outside screen {screen:?}"));
    }
    Ok([(-width, -height), target])
}
