use crate::geometry::{self, Paint, Point, Primitive};
use crate::{badge_color, Color, Error, Frame, PixelFormat, Surface, BACKGROUND, HEIGHT, WIDTH};

#[derive(Clone, Copy)]
struct Span {
    left: i64,
    right: i64,
}
const EMPTY: Span = Span { left: 1, right: 0 };

#[derive(Clone, Copy)]
struct Layer {
    spans: [Span; 4],
    color: Color,
}

fn sqrt(value: i64) -> i64 {
    let mut remainder = value as u64;
    let mut result = 0u64;
    let mut bit = 1u64 << 62;
    while bit > remainder {
        bit >>= 2;
    }
    while bit != 0 {
        if remainder >= result + bit {
            remainder -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result as i64
}

/// Inclusive subpixel span on a horizontal line. Circle boundaries use an
/// integer square root; polygons intersect their oriented half-planes.
fn span(primitive: Primitive, y: i64, unit: i64) -> Span {
    let u = |v: i32| i64::from(v) * unit;
    match primitive {
        Primitive::Rect { x, y: top, w, h } => {
            if y < u(top) || y > u(top + h) {
                EMPTY
            } else {
                Span {
                    left: u(x),
                    right: u(x + w),
                }
            }
        }
        Primitive::Rounded {
            x,
            y: top,
            w,
            h,
            radius,
        } => {
            if y < u(top) || y > u(top + h) {
                return EMPTY;
            }
            let dy = y - y.clamp(u(top + radius), u(top + h - radius));
            let extent = sqrt(u(radius) * u(radius) - dy * dy);
            Span {
                left: u(x + radius) - extent,
                right: u(x + w - radius) + extent,
            }
        }
        Primitive::HalfAnnulus {
            cx,
            cy,
            outer,
            inner,
        } => {
            let dy = y - u(cy);
            let remainder = u(outer) * u(outer) - dy * dy;
            if remainder < 0 {
                return EMPTY;
            }
            let hole = u(inner) * u(inner) - dy * dy;
            let left = if hole > 0 { sqrt(hole - 1) + 1 } else { 0 };
            Span {
                left: u(cx) + left,
                right: u(cx) + sqrt(remainder),
            }
        }
        Primitive::Ring { cx, cy, outer, .. } => {
            // Ring holes are subtracted in hole_span; retain the outer span.
            let dy = y - u(cy);
            let remainder = u(outer) * u(outer) - dy * dy;
            if remainder < 0 {
                EMPTY
            } else {
                let extent = sqrt(remainder);
                Span {
                    left: u(cx) - extent,
                    right: u(cx) + extent,
                }
            }
        }
        Primitive::Polygon(points) => {
            let mut result = Span {
                left: i64::MIN,
                right: i64::MAX,
            };
            for edge in 0..4 {
                let Point(ax, ay) = points[edge];
                let Point(bx, by) = points[(edge + 1) % 4];
                let dx = u(bx - ax);
                let dy = u(by - ay);
                let numerator = dx * (y - u(ay)) + dy * u(ax);
                if dy > 0 {
                    result.right = result.right.min(numerator.div_euclid(dy));
                } else if dy < 0 {
                    result.left = result.left.max(-numerator.div_euclid(-dy));
                } else if numerator < 0 {
                    return EMPTY;
                }
            }
            result
        }
    }
}

fn hole_span(primitive: Primitive, y: i64, unit: i64) -> Span {
    match primitive {
        Primitive::Ring { cx, cy, inner, .. } if inner > 0 => {
            let dy = y - i64::from(cy) * unit;
            let hole = (i64::from(inner) * unit).pow(2) - dy * dy;
            if hole <= 0 {
                return EMPTY;
            }
            let extent = sqrt(hole - 1);
            let center = i64::from(cx) * unit;
            // Return the excluded open disk as an inclusive integer span.
            Span {
                left: center - extent,
                right: center + extent,
            }
        }
        _ => EMPTY,
    }
}

/// Draw the art's bounding rectangle at origin; clip against the surface.
/// The framebuffer is write-only on this path. Scale is pixels per design unit.
pub fn render(
    surface: &mut Surface<'_>,
    origin: (i32, i32),
    scale: u16,
    frame: &Frame,
) -> Result<(), Error> {
    if scale == 0 {
        return Err(Error::ZeroScale);
    }
    let k = i64::from(scale);
    let x0 = i64::from(origin.0).max(0).min(surface.width as i64);
    let y0 = i64::from(origin.1).max(0).min(surface.height as i64);
    let x1 = (i64::from(origin.0) + i64::from(WIDTH) * k)
        .max(0)
        .min(surface.width as i64);
    let y1 = (i64::from(origin.1) + i64::from(HEIGHT) * k)
        .max(0)
        .min(surface.height as i64);
    let bpp = surface.format.bytes_per_pixel();
    for py in y0..y1 {
        let local_y = py - i64::from(origin.1);
        let mut layers = [Layer {
            spans: [EMPTY; 4],
            color: BACKGROUND,
        }; geometry::ART.len() + 9];
        let mut holes = [[EMPTY; 4]; geometry::ART.len() + 9];
        let mut count = 0;
        geometry::shapes(frame, |shape| {
            let color = match shape.paint {
                Paint::Solid(c) => c,
                Paint::BadgeGradient => badge_color(local_y as i32, scale),
            };
            for sy in 0..4 {
                let y = (local_y * 8 + 2 * sy as i64 + 1) * geometry::UNITS;
                layers[count].spans[sy] = span(shape.primitive, y, k * 8);
                holes[count][sy] = hole_span(shape.primitive, y, k * 8);
            }
            layers[count].color = color;
            count += 1;
        });
        for px in x0..x1 {
            let mut rgb = [BACKGROUND.0, BACKGROUND.1, BACKGROUND.2];
            let mut coverage = 0u16;
            let mut color = BACKGROUND;
            let local_x = (px - i64::from(origin.0)) * 8;
            for (layer, hole) in layers[..count].iter().zip(&holes[..count]) {
                if layer.color != color {
                    rgb = blend(rgb, color, coverage.count_ones() as u16);
                    coverage = 0;
                    color = layer.color;
                }
                for sy in 0..4 {
                    let row = layer.spans[sy];
                    for sx in 0..4 {
                        let x = (local_x + 2 * sx as i64 + 1) * geometry::UNITS;
                        if x >= row.left
                            && x <= row.right
                            && !(x >= hole[sy].left && x <= hole[sy].right)
                        {
                            coverage |= 1 << (sy * 4 + sx);
                        }
                    }
                }
            }
            rgb = blend(rgb, color, coverage.count_ones() as u16);
            if matches!(surface.format, PixelFormat::Bgr | PixelFormat::Bgra) {
                rgb.swap(0, 2);
            }
            let offset = py as usize * surface.stride + px as usize * bpp;
            surface.pixels[offset..offset + 3].copy_from_slice(&rgb);
            if bpp == 4 {
                surface.pixels[offset + 3] = 255;
            }
        }
    }
    Ok(())
}

fn blend(under: [u8; 3], Color(r, g, b): Color, coverage: u16) -> [u8; 3] {
    let source = [r, g, b];
    core::array::from_fn(|i| {
        ((u16::from(source[i]) * coverage + u16::from(under[i]) * (16 - coverage) + 8) / 16) as u8
    })
}
