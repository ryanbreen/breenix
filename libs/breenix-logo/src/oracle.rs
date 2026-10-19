//! Independent point-inequality oracle. This module does not call the scanline
//! filler, its square root, span construction, hole subtraction or pixel writer.
//! It shares only the geometry and colour specification with the renderer.
use crate::geometry::{self, Paint, Primitive};
use crate::{badge_color, Color, Frame, BACKGROUND};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected {
    pub color: Color,
    /// True when a partially covered layer contributes a colour boundary. Such
    /// pixels still have an exact expected colour; callers must check them.
    pub antialiased: bool,
}

fn contains(shape: Primitive, px: i64, py: i64, scale: u16) -> bool {
    let px = px * geometry::UNITS;
    let py = py * geometry::UNITS;
    let unit = i64::from(scale) * 8;
    let c = |v: i32| i64::from(v) * unit;
    match shape {
        Primitive::Rect { x, y, w, h } => {
            px >= c(x) && px <= c(x + w) && py >= c(y) && py <= c(y + h)
        }
        Primitive::Rounded { x, y, w, h, radius } => {
            if px < c(x) || px > c(x + w) || py < c(y) || py > c(y + h) {
                return false;
            }
            let dx = if px < c(x + radius) {
                c(x + radius) - px
            } else if px > c(x + w - radius) {
                px - c(x + w - radius)
            } else {
                0
            };
            let dy = if py < c(y + radius) {
                c(y + radius) - py
            } else if py > c(y + h - radius) {
                py - c(y + h - radius)
            } else {
                0
            };
            dx * dx + dy * dy <= c(radius) * c(radius)
        }
        Primitive::HalfAnnulus {
            cx,
            cy,
            outer,
            inner,
        } => {
            let dx = px - c(cx);
            let dy = py - c(cy);
            let distance = dx * dx + dy * dy;
            dx >= 0 && distance <= c(outer) * c(outer) && distance >= c(inner) * c(inner)
        }
        Primitive::Ring {
            cx,
            cy,
            outer,
            inner,
        } => {
            let distance = (px - c(cx)).pow(2) + (py - c(cy)).pow(2);
            distance <= c(outer) * c(outer) && distance >= c(inner) * c(inner)
        }
        Primitive::Polygon(vertices) => (0..4).all(|i| {
            let a = vertices[i];
            let b = vertices[(i + 1) % 4];
            c(b.0 - a.0) * (py - c(a.1)) - c(b.1 - a.1) * (px - c(a.0)) >= 0
        }),
    }
}

/// Local pixel coordinates relative to the art's origin. Includes badge,
/// wordmark, cursor, bar and dots, with exact antialiased edge expectations.
/// `scale = 0` is rejected; negative coordinates return the background.
pub fn expected_at(x: i32, y: i32, scale: u16, frame: &Frame) -> Option<Expected> {
    if scale == 0 {
        return None;
    }
    // Shapes in this table have nonnegative coordinates. Early rejection also
    // bounds the squared distances for callers probing far outside the art.
    if x < 0
        || y < 0
        || i64::from(x) >= i64::from(crate::WIDTH) * i64::from(scale)
        || i64::from(y) >= i64::from(crate::HEIGHT) * i64::from(scale)
    {
        return Some(Expected {
            color: BACKGROUND,
            antialiased: false,
        });
    }
    let mut output = BACKGROUND;
    let mut covered = [false; 16];
    let mut source = BACKGROUND;
    let mut edge = false;
    let resolve = |under: Color, source: Color, covered: &[bool; 16]| {
        let count = covered.iter().filter(|inside| **inside).count() as u32;
        let channel = |src: u8, dst: u8| {
            ((u32::from(src) * count + u32::from(dst) * (16 - count) + 8) / 16) as u8
        };
        (
            Color(
                channel(source.0, under.0),
                channel(source.1, under.1),
                channel(source.2, under.2),
            ),
            count > 0 && count < 16 && source != under,
        )
    };
    geometry::shapes(frame, |shape| {
        let color = match shape.paint {
            Paint::Solid(color) => color,
            Paint::BadgeGradient => badge_color(y, scale),
        };
        if source != color {
            let (next, boundary) = resolve(output, source, &covered);
            output = next;
            edge |= boundary;
            covered = [false; 16];
            source = color;
        }
        for row in 0..4 {
            for column in 0..4 {
                let px = i64::from(x) * 8 + column as i64 * 2 + 1;
                let py = i64::from(y) * 8 + row as i64 * 2 + 1;
                covered[row * 4 + column] |= contains(shape.primitive, px, py, scale);
            }
        }
    });
    let (color, boundary) = resolve(output, source, &covered);
    Some(Expected {
        color,
        antialiased: edge || boundary,
    })
}
