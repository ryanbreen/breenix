use super::*;
use std::vec;
use std::vec::Vec;

fn image(k: u16, frame: &Frame) -> Vec<u8> {
    let width = WIDTH as usize * usize::from(k);
    let height = HEIGHT as usize * usize::from(k);
    let mut pixels = vec![0; width * height * 3];
    let mut surface =
        Surface::new(&mut pixels, width, height, width * 3, PixelFormat::Rgb).unwrap();
    render(&mut surface, (0, 0), k, frame).unwrap();
    pixels
}

fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> Color {
    let i = (y * width + x) * 3;
    Color(pixels[i], pixels[i + 1], pixels[i + 2])
}

#[test]
fn bowls_join_spines_without_a_background_seam() {
    for k in [2, 3, 4] {
        let pixels = image(k, &Frame::default());
        let k = usize::from(k);
        // Cross the badge's upper/lower seams and the wordmark B/R seams.
        for (seam, row, color) in [
            (78, 15, WHITE),
            (78, 48, WHITE),
            (12, 81, WORDMARK),
            (12, 102, WORDMARK),
            (31, 81, WORDMARK),
        ] {
            for x in seam * k - 2..seam * k + 2 {
                assert_eq!(pixel(&pixels, WIDTH as usize * k, x, row * k), color);
            }
        }
        // Bowl counters must expose the gradient rather than fill as disks.
        assert_eq!(
            pixel(&pixels, WIDTH as usize * k, 79 * k, 23 * k),
            badge_color((23 * k) as i32, k as u16)
        );
        assert_eq!(
            pixel(&pixels, WIDTH as usize * k, 13 * k, 86 * k),
            BACKGROUND
        );
    }
}

#[test]
fn gradient_includes_endpoints_at_review_scales() {
    for k in [2, 3, 4] {
        let pixels = image(k, &Frame::default());
        let width = WIDTH as usize * usize::from(k);
        assert_eq!(pixel(&pixels, width, 88 * usize::from(k), 0), INDIGO);
        assert_eq!(
            pixel(&pixels, width, 88 * usize::from(k), 64 * usize::from(k) - 1),
            TEAL
        );
    }
}

#[test]
fn oracle_checks_badge_wordmark_and_antialiased_edges() {
    // Exercise both cursor phases, a partial bar, a full bar, and dot states.
    for k in [2, 3, 4] {
        let frame = Frame {
            cursor_visible: k != 3,
            completed: if k == 4 { 23 } else { 9 },
            total: 23,
            milestones: [
                Milestone::Done,
                Milestone::Done,
                Milestone::Running,
                Milestone::Pending,
                Milestone::Pending,
                Milestone::Pending,
            ],
        };
        let pixels = image(k, &frame);
        let width = WIDTH as usize * usize::from(k);
        let mut badge_edges = 0;
        let mut word_edges = 0;
        let mut dot_edges = 0;
        for y in 0..HEIGHT as usize * usize::from(k) {
            for x in 0..width {
                let expected = oracle::expected_at(x as i32, y as i32, k, &frame).unwrap();
                assert_eq!(
                    pixel(&pixels, width, x, y),
                    expected.color,
                    "k={k} at {x},{y}"
                );
                if expected.antialiased {
                    if y < 64 * usize::from(k) {
                        badge_edges += 1;
                    } else if (80 * usize::from(k)..104 * usize::from(k)).contains(&y) {
                        word_edges += 1;
                    } else if y >= 127 * usize::from(k) {
                        dot_edges += 1;
                    }
                }
            }
        }
        if k == 2 {
            // At (1, 228), both rounded bar layers cover 3/16 samples.
            // Track over field gives (20,26,43), then green gives (29,59,57).
            // Topmost-per-sample averaging would instead give (25,54,51).
            assert_eq!(pixel(&pixels, width, 1, 228), Color(29, 59, 57));
        }
        assert!(badge_edges > 0 && word_edges > 0 && dot_edges > 0);
        let cursor = pixel(&pixels, width, 98 * usize::from(k), 47 * usize::from(k));
        assert_eq!(
            cursor,
            if frame.cursor_visible {
                GREEN
            } else {
                badge_color(47 * i32::from(k), k)
            }
        );
    }
    assert!(cursor_visible(0, true));
    assert!(!cursor_visible(500, true));
    assert!(cursor_visible(1000, true));
    assert!(cursor_visible(750, false));
}

#[test]
fn formats_preserve_channels_alpha_and_stride_padding() {
    let frame = Frame::default();
    let reference = image(2, &frame);
    let width = WIDTH as usize * 2;
    let height = HEIGHT as usize * 2;
    for format in [
        PixelFormat::Rgb,
        PixelFormat::Bgr,
        PixelFormat::Rgba,
        PixelFormat::Bgra,
    ] {
        let bpp = format.bytes_per_pixel();
        let stride = width * bpp + 7;
        let mut output = vec![0xa5; height * stride];
        let mut surface = Surface::new(&mut output, width, height, stride, format).unwrap();
        render(&mut surface, (0, 0), 2, &frame).unwrap();
        for y in 0..height {
            for x in 0..width {
                let Color(r, g, b) = pixel(&reference, width, x, y);
                let expected = if matches!(format, PixelFormat::Bgr | PixelFormat::Bgra) {
                    [b, g, r]
                } else {
                    [r, g, b]
                };
                let index = y * stride + x * bpp;
                assert_eq!(&output[index..index + 3], &expected);
                if bpp == 4 {
                    assert_eq!(output[index + 3], 255);
                }
            }
            assert_eq!(
                &output[y * stride + width * bpp..(y + 1) * stride],
                &[0xa5; 7]
            );
        }
    }
}

#[test]
fn clipped_framebuffers_match_the_visible_region_and_reject_bad_layouts() {
    let frame = Frame::default();
    let reference = image(2, &frame);
    for origin in [(-120, -18), (3, 2), (100, 100), (i32::MIN, i32::MIN)] {
        let mut output = vec![0xa5; 17 * 11 * 3 + 13];
        let mut surface =
            Surface::new(&mut output[..17 * 11 * 3], 17, 11, 51, PixelFormat::Rgb).unwrap();
        render(&mut surface, origin, 2, &frame).unwrap();
        for y in 0..11 {
            for x in 0..17 {
                let local_x = x as i64 - i64::from(origin.0);
                let local_y = y as i64 - i64::from(origin.1);
                let expected = if (0..i64::from(WIDTH) * 2).contains(&local_x)
                    && (0..i64::from(HEIGHT) * 2).contains(&local_y)
                {
                    pixel(
                        &reference,
                        WIDTH as usize * 2,
                        local_x as usize,
                        local_y as usize,
                    )
                } else {
                    Color(0xa5, 0xa5, 0xa5)
                };
                assert_eq!(pixel(&output, 17, x, y), expected);
            }
        }
        assert_eq!(&output[17 * 11 * 3..], &[0xa5; 13]);
    }
    assert!(Surface::new(&mut [0; 5], 2, 1, 6, PixelFormat::Rgb).is_err());
    assert!(Surface::new(&mut [0; 6], 2, 1, 5, PixelFormat::Rgb).is_err());
    assert!(Surface::new(&mut [], usize::MAX, 2, usize::MAX, PixelFormat::Bgra).is_err());
    let mut empty = [];
    let mut surface = Surface::new(&mut empty, 0, 0, 0, PixelFormat::Rgb).unwrap();
    assert_eq!(
        render(&mut surface, (0, 0), 0, &frame),
        Err(Error::ZeroScale)
    );
    assert!(render(&mut surface, (0, 0), 2, &frame).is_ok());
}
