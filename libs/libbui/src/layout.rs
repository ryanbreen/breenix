use alloc::vec::Vec;

use crate::rect::Rect;

/// Lay out `count` items horizontally within `container`.
///
/// Returns a `Vec<Rect>` of `count` items, each `item_width` wide and the
/// full height of the container, spaced `spacing` pixels apart.
pub fn h_stack(container: &Rect, item_width: i32, count: usize, spacing: i32) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(count);
    let mut x = container.x;
    for _ in 0..count {
        rects.push(Rect::new(x, container.y, item_width, container.h));
        x += item_width + spacing;
    }
    rects
}

/// Lay out `count` items vertically within `container`.
///
/// Returns a `Vec<Rect>` of `count` items, each `item_height` tall and the
/// full width of the container, spaced `spacing` pixels apart.
pub fn v_stack(container: &Rect, item_height: i32, count: usize, spacing: i32) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(count);
    let mut y = container.y;
    for _ in 0..count {
        rects.push(Rect::new(container.x, y, container.w, item_height));
        y += item_height + spacing;
    }
    rects
}

/// Center a rectangle of size (`w`, `h`) within `container`.
pub fn center(container: &Rect, w: i32, h: i32) -> Rect {
    Rect::new(
        container.x + (container.w - w) / 2,
        container.y + (container.h - h) / 2,
        w,
        h,
    )
}

/// Right-align a rectangle of size (`w`, `h`) within `container` with a right margin.
pub fn align_right(container: &Rect, w: i32, h: i32, margin: i32) -> Rect {
    Rect::new(
        container.x + container.w - w - margin,
        container.y + (container.h - h) / 2,
        w,
        h,
    )
}
