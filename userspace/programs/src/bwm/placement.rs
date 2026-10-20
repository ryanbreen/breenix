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
