//! The logo's authored geometry, independent of a font or third-party outline.
//! Shapes are closed sets; table order is back to front.
use crate::{Color, Frame, Milestone, AMBER, GREEN, TRACK, WHITE, WORDMARK};

#[derive(Clone, Copy, Debug)]
pub struct Point(pub i32, pub i32);

#[derive(Clone, Copy, Debug)]
pub enum Primitive {
    Rect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    Rounded {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        radius: i32,
    },
    /// Right half of a circular annulus, including the seam at x = cx.
    HalfAnnulus {
        cx: i32,
        cy: i32,
        outer: i32,
        inner: i32,
    },
    /// Convex vertices in positive cross-product order (screen y points down).
    Polygon([Point; 4]),
    Ring {
        cx: i32,
        cy: i32,
        outer: i32,
        inner: i32,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum Paint {
    Solid(Color),
    BadgeGradient,
}

#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub primitive: Primitive,
    pub paint: Paint,
}

const fn rect(x: i32, y: i32, w: i32, h: i32, color: Color) -> Shape {
    Shape {
        primitive: Primitive::Rect { x, y, w, h },
        paint: Paint::Solid(color),
    }
}
const fn bowl(cx: i32, cy: i32, outer: i32, inner: i32, color: Color) -> Shape {
    Shape {
        primitive: Primitive::HalfAnnulus {
            cx,
            cy,
            outer,
            inner,
        },
        paint: Paint::Solid(color),
    }
}
const fn polygon(points: [Point; 4]) -> Shape {
    Shape {
        primitive: Primitive::Polygon(points),
        paint: Paint::Solid(WORDMARK),
    }
}

pub const BADGE_X: i32 = 56;
pub const WORDMARK_Y: i32 = 80;
pub const BAR_Y: i32 = 114;
pub const DOT_Y: i32 = 130;
pub const DOT_X: [i32; 6] = [3, 37, 71, 105, 139, 173];

pub const ART: &[Shape] = &[
    Shape {
        primitive: Primitive::Rounded {
            x: BADGE_X,
            y: 0,
            w: 64,
            h: 64,
            radius: 13,
        },
        paint: Paint::BadgeGradient,
    },
    // Badge B: a shared spine and a wider lower bowl.
    rect(72, 14, 6, 36, WHITE),
    bowl(78, 23, 9, 4, WHITE),
    bowl(78, 39, 11, 5, WHITE),
    // BREENIX, spaced at 26 design units. B and R use the badge's primitives.
    rect(0, 80, 4, 24, WORDMARK),
    bowl(4, 87, 7, 3, WORDMARK),
    bowl(4, 96, 8, 4, WORDMARK),
    rect(26, 80, 4, 24, WORDMARK),
    bowl(30, 87, 7, 3, WORDMARK),
    polygon([Point(30, 91), Point(35, 91), Point(46, 104), Point(41, 104)]),
    rect(52, 80, 4, 24, WORDMARK),
    rect(52, 80, 18, 4, WORDMARK),
    rect(52, 90, 15, 4, WORDMARK),
    rect(52, 100, 18, 4, WORDMARK),
    rect(78, 80, 4, 24, WORDMARK),
    rect(78, 80, 18, 4, WORDMARK),
    rect(78, 90, 15, 4, WORDMARK),
    rect(78, 100, 18, 4, WORDMARK),
    rect(104, 80, 4, 24, WORDMARK),
    polygon([
        Point(104, 80),
        Point(108, 80),
        Point(122, 104),
        Point(118, 104),
    ]),
    rect(118, 80, 4, 24, WORDMARK),
    rect(134, 80, 4, 24, WORDMARK),
    rect(130, 80, 12, 4, WORDMARK),
    rect(130, 100, 12, 4, WORDMARK),
    polygon([
        Point(156, 80),
        Point(161, 80),
        Point(176, 104),
        Point(171, 104),
    ]),
    polygon([
        Point(171, 80),
        Point(176, 80),
        Point(161, 104),
        Point(156, 104),
    ]),
];

/// Geometry enumeration is part of the specification shared with the oracle.
pub fn shapes(frame: &Frame, mut emit: impl FnMut(Shape)) {
    for shape in ART {
        emit(*shape);
    }
    if frame.cursor_visible {
        emit(rect(94, 45, 13, 5, GREEN));
    }
    emit(Shape {
        primitive: Primitive::Rounded {
            x: 0,
            y: BAR_Y,
            w: 176,
            h: 4,
            radius: 2,
        },
        paint: Paint::Solid(TRACK),
    });
    let fill = if frame.total == 0 {
        0
    } else {
        176 * u32::from(frame.completed.min(frame.total)) / u32::from(frame.total)
    } as i32;
    if fill > 0 {
        emit(Shape {
            primitive: Primitive::Rounded {
                x: 0,
                y: BAR_Y,
                w: fill,
                h: 4,
                radius: (fill / 2).min(2),
            },
            paint: Paint::Solid(GREEN),
        });
    }
    for (cx, milestone) in DOT_X.into_iter().zip(frame.milestones) {
        let (inner, color) = match milestone {
            Milestone::Pending => (2, TRACK),
            Milestone::Running => (0, AMBER),
            Milestone::Done => (0, GREEN),
        };
        emit(Shape {
            primitive: Primitive::Ring {
                cx,
                cy: DOT_Y,
                outer: 3,
                inner,
            },
            paint: Paint::Solid(color),
        });
    }
}
