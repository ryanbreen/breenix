//! The logo's authored geometry, independent of a font or third-party outline.
//! Coordinates are thirds of a design unit, preserving the scaled badge bowls.
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

// The badge B uses the original 36-unit-high construction.
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
// Wordmark bowls retain the badge radii at 2/3 scale. Straight shoulders
// extend them to the wider wordmark proportions; B and R share the upper bowl.
const fn word_upper_bowl(x: i32) -> Shape {
    Shape {
        primitive: Primitive::HalfAnnulus {
            cx: x * 3 + 26,
            cy: 86 * 3,
            outer: 18,
            inner: 8,
        },
        paint: Paint::Solid(WORDMARK),
    }
}
const fn word_shoulder(x: i32, y: i32, h: i32) -> Shape {
    Shape {
        primitive: Primitive::Rect {
            x: x * 3,
            y,
            w: 26,
            h,
        },
        paint: Paint::Solid(WORDMARK),
    }
}

const fn rect(x: i32, y: i32, w: i32, h: i32, color: Color) -> Shape {
    Shape {
        primitive: design(Primitive::Rect { x, y, w, h }),
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
    // Optical spacing follows the contours, especially around I's inset stem.
    // Ink bounds: B 19..35, R 42..58, E 64..80, E 84..100,
    // N 104..120, I 126..136, X 139..157. Centre: badge/bar axis x=88.
    rect(19, 80, 4, 24, WORDMARK),
    word_shoulder(19, 240, 10),
    word_upper_bowl(19),
    word_shoulder(19, 266, 14),
    Shape {
        primitive: Primitive::HalfAnnulus {
            cx: 83,
            cy: 290,
            outer: 22,
            inner: 10,
        },
        paint: Paint::Solid(WORDMARK),
    },
    word_shoulder(19, 300, 12),
    rect(42, 80, 4, 24, WORDMARK),
    word_shoulder(42, 240, 10),
    word_upper_bowl(42),
    word_shoulder(42, 266, 10),
    polygon([Point(46, 91), Point(51, 91), Point(58, 104), Point(53, 104)]),
    rect(64, 80, 4, 24, WORDMARK),
    rect(64, 80, 16, 4, WORDMARK),
    rect(64, 90, 13, 4, WORDMARK),
    rect(64, 100, 16, 4, WORDMARK),
    rect(84, 80, 4, 24, WORDMARK),
    rect(84, 80, 16, 4, WORDMARK),
    rect(84, 90, 13, 4, WORDMARK),
    rect(84, 100, 16, 4, WORDMARK),
    rect(104, 80, 4, 24, WORDMARK),
    polygon([
        Point(104, 80),
        Point(108, 80),
        Point(120, 104),
        Point(116, 104),
    ]),
    rect(116, 80, 4, 24, WORDMARK),
    rect(129, 80, 4, 24, WORDMARK),
    rect(126, 80, 10, 4, WORDMARK),
    rect(126, 100, 10, 4, WORDMARK),
    polygon([
        Point(139, 80),
        Point(144, 80),
        Point(157, 104),
        Point(152, 104),
    ]),
    polygon([
        Point(152, 80),
        Point(157, 80),
        Point(144, 104),
        Point(139, 104),
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
