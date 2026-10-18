//! The logo's authored geometry, independent of a font or third-party outline.
//! Coordinates are thirds of a design unit, preserving an exact 2/3-size B.
//! Shapes are closed sets; table order is back to front. Adjacent shapes with
//! the same paint form one coverage layer (their sample masks are unioned).
use crate::{Color, Frame, Milestone, AMBER, GREEN, PENDING, TRACK, WHITE, WORDMARK};

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

pub const UNITS: i64 = 3;

const fn design(primitive: Primitive) -> Primitive {
    match primitive {
        Primitive::Rect { x, y, w, h } => Primitive::Rect {
            x: x * 3,
            y: y * 3,
            w: w * 3,
            h: h * 3,
        },
        Primitive::Rounded { x, y, w, h, radius } => Primitive::Rounded {
            x: x * 3,
            y: y * 3,
            w: w * 3,
            h: h * 3,
            radius: radius * 3,
        },
        Primitive::HalfAnnulus {
            cx,
            cy,
            outer,
            inner,
        } => Primitive::HalfAnnulus {
            cx: cx * 3,
            cy: cy * 3,
            outer: outer * 3,
            inner: inner * 3,
        },
        Primitive::Ring {
            cx,
            cy,
            outer,
            inner,
        } => Primitive::Ring {
            cx: cx * 3,
            cy: cy * 3,
            outer: outer * 3,
            inner: inner * 3,
        },
        Primitive::Polygon(p) => Primitive::Polygon([
            Point(p[0].0 * 3, p[0].1 * 3),
            Point(p[1].0 * 3, p[1].1 * 3),
            Point(p[2].0 * 3, p[2].1 * 3),
            Point(p[3].0 * 3, p[3].1 * 3),
        ]),
    }
}

// The badge and wordmark use one B, at size 3/3 and 2/3 respectively.
const fn letter_b(x: i32, y: i32, size: i32, color: Color) -> [Shape; 3] {
    [
        Shape {
            primitive: Primitive::Rect {
                x: x * 3,
                y: y * 3,
                w: 6 * size,
                h: 36 * size,
            },
            paint: Paint::Solid(color),
        },
        Shape {
            primitive: Primitive::HalfAnnulus {
                cx: x * 3 + 6 * size,
                cy: y * 3 + 9 * size,
                outer: 9 * size,
                inner: 4 * size,
            },
            paint: Paint::Solid(color),
        },
        Shape {
            primitive: Primitive::HalfAnnulus {
                cx: x * 3 + 6 * size,
                cy: y * 3 + 25 * size,
                outer: 11 * size,
                inner: 5 * size,
            },
            paint: Paint::Solid(color),
        },
    ]
}
const BADGE_B: [Shape; 3] = letter_b(72, 14, 3, WHITE);
const WORD_B: [Shape; 3] = letter_b(8, 80, 2, WORDMARK);

const fn rect(x: i32, y: i32, w: i32, h: i32, color: Color) -> Shape {
    Shape {
        primitive: design(Primitive::Rect { x, y, w, h }),
        paint: Paint::Solid(color),
    }
}
const fn bowl(cx: i32, cy: i32, outer: i32, inner: i32, color: Color) -> Shape {
    Shape {
        primitive: design(Primitive::HalfAnnulus {
            cx,
            cy,
            outer,
            inner,
        }),
        paint: Paint::Solid(color),
    }
}
const fn polygon(points: [Point; 4]) -> Shape {
    Shape {
        primitive: design(Primitive::Polygon(points)),
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
        primitive: design(Primitive::Rounded {
            x: BADGE_X,
            y: 0,
            w: 64,
            h: 64,
            radius: 13,
        }),
        paint: Paint::BadgeGradient,
    },
    BADGE_B[0],
    BADGE_B[1],
    BADGE_B[2],
    // Optical spacing: compact round-to-straight pairs; bring I and X in.
    // Ink bounds: B 8..19 1/3, R 27..47, E 53..71, E 79..97,
    // N 105..123, I 129..141, X 147..167.
    WORD_B[0],
    WORD_B[1],
    WORD_B[2],
    rect(27, 80, 4, 24, WORDMARK),
    bowl(31, 87, 7, 3, WORDMARK),
    polygon([Point(31, 91), Point(36, 91), Point(47, 104), Point(42, 104)]),
    rect(53, 80, 4, 24, WORDMARK),
    rect(53, 80, 18, 4, WORDMARK),
    rect(53, 90, 15, 4, WORDMARK),
    rect(53, 100, 18, 4, WORDMARK),
    rect(79, 80, 4, 24, WORDMARK),
    rect(79, 80, 18, 4, WORDMARK),
    rect(79, 90, 15, 4, WORDMARK),
    rect(79, 100, 18, 4, WORDMARK),
    rect(105, 80, 4, 24, WORDMARK),
    polygon([
        Point(105, 80),
        Point(109, 80),
        Point(123, 104),
        Point(119, 104),
    ]),
    rect(119, 80, 4, 24, WORDMARK),
    rect(133, 80, 4, 24, WORDMARK),
    rect(129, 80, 12, 4, WORDMARK),
    rect(129, 100, 12, 4, WORDMARK),
    polygon([
        Point(147, 80),
        Point(152, 80),
        Point(167, 104),
        Point(162, 104),
    ]),
    polygon([
        Point(162, 80),
        Point(167, 80),
        Point(152, 104),
        Point(147, 104),
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
        primitive: design(Primitive::Rounded {
            x: 0,
            y: BAR_Y,
            w: 176,
            h: 4,
            radius: 2,
        }),
        paint: Paint::Solid(TRACK),
    });
    let fill = if frame.total == 0 {
        0
    } else {
        176 * u32::from(frame.completed.min(frame.total)) / u32::from(frame.total)
    } as i32;
    if fill > 0 {
        emit(Shape {
            primitive: design(Primitive::Rounded {
                x: 0,
                y: BAR_Y,
                w: fill,
                h: 4,
                radius: (fill / 2).min(2),
            }),
            paint: Paint::Solid(GREEN),
        });
    }
    for (cx, milestone) in DOT_X.into_iter().zip(frame.milestones) {
        let (inner, color) = match milestone {
            Milestone::Pending => (2, PENDING),
            Milestone::Running => (0, AMBER),
            Milestone::Done => (0, GREEN),
        };
        emit(Shape {
            primitive: design(Primitive::Ring {
                cx,
                cy: DOT_Y,
                outer: 3,
                inner,
            }),
            paint: Paint::Solid(color),
        });
    }
}
