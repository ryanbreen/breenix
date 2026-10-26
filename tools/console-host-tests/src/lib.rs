//! Host tests for the screen console's grid, its font size rule, its drawing
//! against the read-back oracle, and the x86 write-combining page-table walk.
#![cfg(test)]
#![feature(allocator_api)]

#[path = "../../../kernel/src/console/boxdraw.rs"]
mod boxdraw;
#[path = "../../../kernel/src/console/grid.rs"]
pub mod grid;
#[path = "../../../kernel/src/arch_impl/x86_64/pagewalk.rs"]
mod pagewalk;
#[path = "../../../kernel/src/console/raster.rs"]
mod raster;

mod grid_tests {
    use super::grid::{Grid, BLANK, MAX_COLS, MAX_ROWS};

    fn grid(cols: usize, rows: usize) -> Box<Grid> {
        let mut grid = Box::new(Grid::new());
        let size = cols.min(MAX_COLS) * rows.min(MAX_ROWS);
        grid.set_storage(vec![BLANK; size].leak(), vec![BLANK; size].leak());
        grid.set_size(cols, rows);
        grid
    }

    fn text(grid: &Grid, row: usize) -> String {
        grid.row(row)
            .iter()
            .map(|cell| cell.character())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn native_boot_resize_preserves_text_history_cursor_and_pen() {
        let mut g=Grid::new();
        g.set_storage(vec![BLANK;8*6].leak(),vec![BLANK;8*6].leak());
        g.set_size(4,2);
        g.set_history(vec![BLANK;super::grid::HISTORY_LINES*8].leak());
        g.write_kernel(b"one\ntwo\n",false);
        g.write(b"\x1b[31mR");
        let red=g.row(1)[0];
        assert_eq!(text(&g,0),"two");
        assert_eq!(g.cursor(),(1,1));
        g.resize_boot(3,4).unwrap();
        assert_eq!((g.cols(),g.rows()),(3,4));
        assert_eq!(text(&g,0),"two");
        assert_eq!(text(&g,1),"R");
        assert_eq!(g.row(1)[0],red);
        assert_eq!(text(&g,2),"");
        assert_eq!(g.cursor(),(1,1));
        g.scroll_view(isize::MAX);
        assert_eq!(g.view_row(0).iter().map(|c|c.character()).collect::<String>(),"one");
        g.view_live();
        g.write(b"S");
        assert_eq!(text(&g,1),"RS");
        assert_eq!(g.row(1)[1].colours,red.colours);
        assert!(g.resize_boot(9,9).is_err());
        assert_eq!((g.cols(),g.rows()),(3,4));
        assert_eq!(text(&g,1),"RS");
    }

    #[test]
    fn kernel_records_escape_application_modes_and_control_injection() {
        let mut g = grid(64, 6);
        g.write(b"main\x1b[?1049h\x1b[8mhidden\x1b[2;3r\x1b[?7l\x1b[");
        g.write_kernel(b"WARN: \x1bc\x1b[?1049h\x1b[8mvisible\x08\x7f\n", true);
        assert_eq!(text(&g, 0), "main");
        assert_eq!(text(&g, 1), "WARN: c[?1049h[8mvisible");
        assert!(g.row(1).iter().all(|cell| *cell
            == super::grid::Cell {
                glyph: cell.glyph,
                ..super::grid::BLANK
            }));
        // The interrupted TTY CSI is still pending; the record did not parse it.
        g.write(b"31mR");
        assert_eq!(text(&g, 2), "R");
        assert_eq!(g.row(2)[0].colours, 1);
        // Returning from the alternate screen cannot discard the warning.
        g.write(b"\x1b[?1049l");
        assert_eq!(text(&g, 1), "WARN: c[?1049h[8mvisible");
        g.write(b"\x1b[8m");
        g.write_kernel(b"WARN: still visible\n", true);
        assert_eq!(text(&g, 3), "WARN: still visible");
        assert_eq!(g.row(3)[0].flags, 0);
        g.write(b"X");
        assert_eq!(g.row(4)[0].flags, super::grid::INVISIBLE);
    }

    #[test]
    fn kernel_fragments_use_default_pen_and_full_screen_scrolling() {
        let mut g = grid(8, 4);
        g.write(b"\x1b[2;3r\x1b[4;1H\x1b[8m\x1b[?7l");
        g.write_kernel(b"WARN:", true);
        g.write_kernel(b" abcdef\n", false);
        assert_eq!(
            [text(&g, 0), text(&g, 1), text(&g, 2), text(&g, 3)],
            ["", "WARN: ab", "cdef", ""]
        );
        assert_eq!(g.row(1)[0].flags, 0);
        assert_eq!(g.row(2)[0].flags, 0);
        // The process's scrolling region and autowrap setting are restored.
        g.write(b"\x1b[3;1HA\x1b[K\nB");
        assert_eq!(text(&g, 1), "A");
        g.write(b"\x1b[1;8Hxy");
        assert_eq!(text(&g, 0), "       y");
    }

    #[test]
    fn erased_inserted_and_scrolled_cells_keep_colours_without_attributes() {
        let mut g = grid(8, 3);
        g.write(b"\x1b[31;44;4;7;8m\x1b[2J");
        let blank = super::grid::Cell {
            colours: 0x41,
            ..super::grid::BLANK
        };
        assert!((0..3).all(|row| g.row(row).iter().all(|&cell| cell == blank)));
        g.write(b"x\x1b[H\x1b[@");
        assert_eq!(g.row(0)[0], blank);
        g.write(b"\x1b[3;1H\n");
        assert!(g.row(2).iter().all(|&cell| cell == blank));
    }

    #[test]
    fn split_utf8_and_control_sequences_keep_their_state() {
        let bytes = "\x1b[31;4mĀé\x1b[2;3H┌\x1b[?25l".as_bytes();
        for split in 0..=bytes.len() {
            let mut g = grid(10, 3);
            g.write(&bytes[..split]);
            g.write(&bytes[split..]);
            assert_eq!(text(&g, 0), "Āé", "split {split}");
            assert_eq!(text(&g, 1), "  ┌", "split {split}");
            assert_eq!(g.row(0)[0].colours, 1);
            assert_eq!(g.row(0)[1].flags, super::grid::UNDERLINE);
            assert!(!g.cursor_visible());
        }
        let mut g = grid(20, 3);
        g.write(b"\xe2\x82");
        assert_eq!(text(&g, 0), "");
        g.write(b"X\xed\xa0\x80\xf4\x90\x80\x80");
        g.write(b"\x1b[31\x18m\x1b]ignored\x1b\\Y\x1b[3;4H\x1b[6");
        let mut reply = Vec::new();
        g.write_with_reply(b"n", |bytes| reply.extend_from_slice(bytes));
        assert_eq!(text(&g, 0), "?X??mY");
        assert_eq!(reply, b"\x1b[3;4R");
    }

    #[test]
    fn parameter_overflow_saturates_and_ignores_excess_parameters() {
        let mut g = grid(10, 3);
        g.write(b"\x1b[999999999999999999999999;999999999999999H!");
        assert_eq!(g.cursor(), (2, 9));
        assert_eq!(text(&g, 2), "         !");
        g.write(b"\x1b[H\x1b[0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;31;999;32mR");
        assert_eq!(g.row(0)[0].colours, 1);
        // No overflow digits or trailing unknown sequence bytes reach the grid.
        g.write(b"\x1b[999999999999999999999999zX");
        assert_eq!(text(&g, 0), "RX");
    }

    #[test]
    fn pending_wrap_scrolls_only_the_region_and_survives_sgr() {
        let mut g = grid(4, 5);
        g.write(b"AAAA\x1b[2;1HBBBB\x1b[3;1HCCCC\x1b[4;1HDDDD\x1b[5;1HEEEE");
        g.write(b"\x1b[2;4r\x1b[4;1Hwxyz\x1b[31m");
        assert_eq!(text(&g, 3), "wxyz");
        g.write(b"Q");
        assert_eq!(
            (0..5).map(|r| text(&g, r)).collect::<Vec<_>>(),
            ["AAAA", "CCCC", "wxyz", "Q", "EEEE"]
        );
        assert_eq!(g.cursor(), (3, 1));
        assert_eq!(g.row(3)[0].colours, 1);
        g.write(b"\x1b[4GZ\x1b[4G!");
        assert_eq!(text(&g, 3), "Q  !");
        g.write(b"\x1b[?7lxy");
        assert_eq!(text(&g, 3), "Q  y");
    }

    #[test]
    fn scrolling_keeps_the_newest_rows() {
        let mut g = grid(10, 3);
        g.write(b"a\r\nb\r\nc\r\nd");
        assert_eq!([text(&g, 0), text(&g, 1), text(&g, 2)], ["b", "c", "d"]);
        assert_eq!(g.cursor(), (2, 1));
        g.write(b"\r\ne\r\nf\r\ng\r\nh");
        assert_eq!([text(&g, 0), text(&g, 1), text(&g, 2)], ["f", "g", "h"]);
    }

    #[test]
    fn a_character_in_the_last_column_wraps_on_the_next() {
        let mut g = grid(4, 3);
        g.write(b"abcd");
        assert_eq!((text(&g, 0), g.cursor()), ("abcd".into(), (0, 3)));
        g.write(b"e");
        assert_eq!((text(&g, 1), g.cursor()), ("e".into(), (1, 1)));
        // A CR after the last column returns to it without a new row.
        g.write(b"\r\nwxyz\rq");
        assert_eq!(text(&g, 2), "qxyz");
    }

    #[test]
    fn line_feed_alone_keeps_the_column() {
        let mut g = grid(10, 3);
        g.write(b"ab\ncd");
        assert_eq!([text(&g, 0), text(&g, 1)], ["ab", "  cd"]);
    }

    #[test]
    fn backspace_tab_and_erase_sequences() {
        let mut g = grid(20, 3);
        g.write(b"abc\x08\x1b[K");
        assert_eq!(text(&g, 0), "ab");
        g.write(b"\tx");
        assert_eq!(text(&g, 0), "ab      x");
        g.write(b"\x1b[2;3Hy\x1b[?25l\x1b[1;31mz");
        assert_eq!(text(&g, 1), "  yz");
        g.write(b"\x1b[1;5H\x1b[1K");
        assert_eq!(text(&g, 0), "        x");
        g.write(b"\x1b[2J");
        assert!((0..3).all(|row| text(&g, row).is_empty()));
    }

    #[test]
    fn a_log_line_starts_its_own_row_only_mid_line() {
        let mut g = grid(20, 4);
        g.write(b"login: ");
        g.begin_line();
        g.write(b"WARN: x\r\n");
        g.begin_line();
        g.write(b"root");
        assert_eq!(
            [text(&g, 0), text(&g, 1), text(&g, 2)],
            ["login:", "WARN: x", "root"]
        );
    }

    #[test]
    fn a_utf8_character_takes_one_cell() {
        let mut g = grid(10, 2);
        g.write("é!".as_bytes());
        assert_eq!((text(&g, 0), g.cursor()), ("é!".into(), (0, 2)));
    }

    #[test]
    fn a_grid_larger_than_its_storage_is_cut_to_it() {
        let mut g = grid(10_000, 10_000);
        assert_eq!(
            (g.cols(), g.rows()),
            (super::grid::MAX_COLS, super::grid::MAX_ROWS)
        );
        g.write(&vec![
            b'x';
            super::grid::MAX_COLS * super::grid::MAX_ROWS + 1
        ]);
        assert_eq!(text(&g, super::grid::MAX_ROWS - 1), "x");
    }

    #[test]
    fn every_write_changes_the_generation() {
        let mut g = grid(10, 2);
        let before = g.generation();
        g.write(b"x");
        assert_ne!(g.generation(), before);
    }

    /// Forward `writes` in turn; the bytes forwarded and the replies kept.
    fn forward(g: &mut Grid, writes: &[&[u8]], keep: bool) -> (Vec<u8>, Vec<u8>) {
        let (mut forwarded, mut replies) = (Vec::new(), Vec::new());
        for bytes in writes {
            let mut out = vec![0; bytes.len() + super::grid::HELD];
            let n = g.write_forwarding(bytes, |reply| {
                if keep {
                    replies.extend_from_slice(reply);
                }
                keep
            }, &mut out);
            forwarded.extend_from_slice(&out[..n]);
        }
        (forwarded, replies)
    }

    #[test]
    fn an_answered_request_is_not_forwarded_even_when_split() {
        let mut g = grid(20, 3);
        let (forwarded, replies) = forward(&mut g, &[b"ab\x1b[", b"6ncd\x1b[5n"], true);
        assert_eq!(forwarded, b"abcd");
        assert_eq!(replies, b"\x1b[1;3R\x1b[0n");
    }

    #[test]
    fn other_sequences_are_forwarded_whole_and_in_order() {
        let mut g = grid(20, 3);
        let (forwarded, replies) = forward(&mut g, &[b"a\x1b[3", b"1mb\x1b]0;t\x07c\x1b(Bd\x1b[?6n"], true);
        // The grid does not answer a private-marker DSR, so it is forwarded.
        assert_eq!(forwarded, b"a\x1b[31mb\x1b]0;t\x07c\x1b(Bd\x1b[?6n");
        assert!(replies.is_empty());
        assert_eq!(text(&g, 0), "abcd");
    }

    #[test]
    fn a_reply_not_kept_leaves_its_request_forwarded() {
        let mut g = grid(20, 3);
        let (forwarded, replies) = forward(&mut g, &[b"x\x1b[6n"], false);
        assert_eq!(forwarded, b"x\x1b[6n");
        assert!(replies.is_empty());
    }

    #[test]
    fn a_request_too_long_to_hold_is_forwarded_and_not_answered() {
        let mut g = grid(20, 3);
        let request = [&b"\x1b["[..], &[b'0'; super::grid::HELD][..], b"6n"].concat();
        let (forwarded, replies) = forward(&mut g, &[&request[..10], &request[10..], b"\x1b[5n"], true);
        assert_eq!(forwarded, request);
        assert_eq!(replies, b"\x1b[0n");
    }

    #[test]
    fn an_interrupted_sequence_is_forwarded_and_controls_inside_a_request_still_go() {
        for bytes in [b"\x1b[6\x1b[5nX".as_slice(), b"\x1b[6\x1b[5\rnX".as_slice()] {
            for split in 0..=bytes.len() {
                let mut g = grid(20, 3);
                let (forwarded, replies) = forward(&mut g, &[&bytes[..split], &bytes[split..]], true);
                let expected = if bytes.contains(&b'\r') { b"\x1b[6\x18\rX".as_slice() } else { b"\x1b[6\x18X".as_slice() };
                assert_eq!(forwarded, expected, "split {split}");
                assert_eq!(replies, b"\x1b[0n");
                let mut serial = grid(20, 3);
                serial.write(&forwarded);
                assert_eq!(text(&serial, 0), "X", "split {split}");
                assert_eq!(text(&g, 0), "X", "split {split}");
            }
        }
    }
}

mod raster_tests {
    use super::grid::{Cell, BLANK};
    use super::raster::{clear, draw_row, expected_colour, Font, Layout, PALETTE};

    fn cells(bytes: &[u8]) -> Vec<Cell> {
        bytes
            .iter()
            .map(|&byte| Cell {
                glyph: u16::from(byte),
                ..BLANK
            })
            .collect()
    }
    use noto_sans_mono_bitmap::RasterHeight;

    const SIZE_16: Font = Font::new(RasterHeight::Size16);

    /// A framebuffer `cols` cells and `extra` pixels wide, `rows` text rows and
    /// `extra_lines` pixel rows tall, with `pad` bytes after each pixel row.
    fn layout(
        font: Font,
        bpp: usize,
        cols: usize,
        extra: usize,
        rows: usize,
        extra_lines: usize,
        pad: usize,
    ) -> Layout {
        let width = cols * font.glyph_width() + extra;
        Layout {
            width,
            height: rows * font.line_height() + extra_lines,
            bytes_per_pixel: bpp,
            stride_bytes: width * bpp + pad,
            reserved: if bpp == 4 { 0xff } else { 0 },
            bgr: bpp == 4,
            font,
        }
    }

    fn printable() -> Vec<Cell> {
        cells(&(0x20..0x7f).collect::<Vec<u8>>())
    }

    /// Each pixel of each band, against the oracle; padding is left alone.
    fn check(font: Font, bpp: usize) {
        let ascii = printable();
        let layout = layout(font, bpp, ascii.len(), 5, 3, 7, 12);
        let mut frame = vec![0xaa; layout.bytes()];
        let rows: [(Vec<Cell>, Option<usize>); 3] = [
            (ascii, Some(10)),
            (cells(b"metal-login-ok 42"), None),
            (cells(b"#\x7f\x01"), Some(2)),
        ];
        for (row, (text, cursor)) in rows.iter().enumerate() {
            let mut cells = text.to_vec();
            cells.resize(layout.cols(), BLANK);
            draw_row(&mut frame, &layout, row, &cells, *cursor);
        }
        for (row, (text, cursor)) in rows.iter().enumerate() {
            let mut cells = text.to_vec();
            cells.resize(layout.cols(), BLANK);
            for y in layout.band(row) {
                let line = &frame[layout.line(y)];
                for (x, pixel) in line.chunks_exact(bpp).enumerate() {
                    let expected =
                        expected_colour(font, &cells, *cursor, x, y - layout.band(row).start);
                    assert!(
                        layout.pixel_is(pixel, expected),
                        "size {} row {row} x {x} y {y}: {pixel:?} vs {expected:?}",
                        font.size()
                    );
                }
                let padding = layout.line(y).end..(y + 1) * layout.stride_bytes;
                assert!(frame[padding].iter().all(|&byte| byte == 0xaa));
            }
        }
    }

    #[test]
    fn latin1_space_and_soft_hyphen_have_their_own_shapes() {
        for size in Font::SIZES {
            let font = Font::new(size);
            for flags in [0, super::grid::BOLD] {
                for (glyph, reference) in [(0x00a0, b' '), (0x00ad, b'-')] {
                    let layout = layout(font, 4, 1, 0, 1, 0, 0);
                    let mut actual = vec![0; layout.bytes()];
                    let mut expected = vec![0; layout.bytes()];
                    draw_row(
                        &mut actual,
                        &layout,
                        0,
                        &[Cell {
                            glyph,
                            flags,
                            ..BLANK
                        }],
                        None,
                    );
                    draw_row(
                        &mut expected,
                        &layout,
                        0,
                        &[Cell {
                            glyph: u16::from(reference),
                            flags,
                            ..BLANK
                        }],
                        None,
                    );
                    assert_eq!(actual, expected, "size {} glyph {glyph:x}", font.size());
                }
            }
        }
    }

    #[test]
    fn double_junction_pixels_match_independent_lane_rectangles() {
        // Explicit stroke rectangles, independent of the production arm table
        // and junction endpoint calculation. Check ink AND the open lane gaps.
        for size in Font::SIZES {
            let font = Font::new(size);
            let layout = layout(font, 4, 1, 0, 1, 0, 0);
            let (w, h) = (font.glyph_width(), font.line_height());
            let (cx, cy) = (w / 2, h / 2);
            let thickness = h / 16;
            let (xl, xr, yt, yb) = (
                cx - thickness - 1,
                cx + thickness + 1,
                cy - thickness - 1,
                cy + thickness + 1,
            );
            // Rectangles are inclusive, with thickness growing right/down.
            let t = thickness - 1;
            let vleft = (xl, 0, xl + t, h - 1);
            let vright = (xr, 0, xr + t, h - 1);
            let htop = (0, yt, w - 1, yt + t);
            let hbottom = (0, yb, w - 1, yb + t);
            let hright = vec![(xr, yt, w - 1, yt + t), (xr, yb, w - 1, yb + t)];
            let hleft = vec![(0, yt, xl + t, yt + t), (0, yb, xl + t, yb + t)];
            let vup = vec![(xl, 0, xl + t, yt + t), (xr, 0, xr + t, yt + t)];
            let vdown = vec![(xl, yb, xl + t, h - 1), (xr, yb, xr + t, h - 1)];
            let cases = [
                (
                    0x2552,
                    vec![
                        (cx, yt, w - 1, yt + t),
                        (cx, yb, w - 1, yb + t),
                        (cx, yt, cx + t, h - 1),
                    ],
                ),
                (
                    0x2553,
                    vec![
                        (xl, cy, w - 1, cy + t),
                        (xl, cy, xl + t, h - 1),
                        (xr, cy, xr + t, h - 1),
                    ],
                ),
                (
                    0x2555,
                    vec![
                        (0, yt, cx + t, yt + t),
                        (0, yb, cx + t, yb + t),
                        (cx, yt, cx + t, h - 1),
                    ],
                ),
                (
                    0x2556,
                    vec![
                        (0, cy, xr + t, cy + t),
                        (xl, cy, xl + t, h - 1),
                        (xr, cy, xr + t, h - 1),
                    ],
                ),
                (
                    0x2558,
                    vec![
                        (cx, yt, w - 1, yt + t),
                        (cx, yb, w - 1, yb + t),
                        (cx, 0, cx + t, yb + t),
                    ],
                ),
                (
                    0x2559,
                    vec![
                        (xl, cy, w - 1, cy + t),
                        (xl, 0, xl + t, cy + t),
                        (xr, 0, xr + t, cy + t),
                    ],
                ),
                (
                    0x255b,
                    vec![
                        (0, yt, cx + t, yt + t),
                        (0, yb, cx + t, yb + t),
                        (cx, 0, cx + t, yb + t),
                    ],
                ),
                (
                    0x255c,
                    vec![
                        (0, cy, xr + t, cy + t),
                        (xl, 0, xl + t, cy + t),
                        (xr, 0, xr + t, cy + t),
                    ],
                ),
                (0x256a, vec![htop, hbottom, (cx, 0, cx + t, h - 1)]),
                (0x256b, vec![vleft, vright, (0, cy, w - 1, cy + t)]),
                (0x255f, vec![vleft, vright, (xr, cy, w - 1, cy + t)]),
                (0x2562, vec![vleft, vright, (0, cy, xl + t, cy + t)]),
                (
                    0x2560,
                    [
                        vec![vleft],
                        vup[1..].to_vec(),
                        vdown[1..].to_vec(),
                        hright.clone(),
                    ]
                    .concat(),
                ),
                (
                    0x2563,
                    [
                        vec![vright],
                        vup[..1].to_vec(),
                        vdown[..1].to_vec(),
                        hleft.clone(),
                    ]
                    .concat(),
                ),
                (0x2564, vec![htop, hbottom, (cx, yb, cx + t, h - 1)]),
                (0x2567, vec![htop, hbottom, (cx, 0, cx + t, yt + t)]),
                (
                    0x2566,
                    [
                        vec![htop],
                        hleft[1..].to_vec(),
                        hright[1..].to_vec(),
                        vdown.clone(),
                    ]
                    .concat(),
                ),
                (
                    0x2569,
                    [
                        vec![hbottom],
                        hleft[..1].to_vec(),
                        hright[..1].to_vec(),
                        vup.clone(),
                    ]
                    .concat(),
                ),
                (0x256c, [hleft, hright, vup, vdown].concat()),
            ];
            for (glyph, rectangles) in cases {
                let mut frame = vec![0; layout.bytes()];
                draw_row(&mut frame, &layout, 0, &[Cell { glyph, ..BLANK }], None);
                for y in 0..h {
                    for x in 0..w {
                        let ink = rectangles
                            .iter()
                            .any(|&(x0, y0, x1, y1)| x >= x0 && x <= x1 && y >= y0 && y <= y1);
                        let pixel = &frame[y * layout.stride_bytes + x * 4..][..4];
                        assert!(
                            layout.pixel_is(pixel, PALETTE[if ink { 7 } else { 0 }]),
                            "size {} glyph {glyph:x} ({x},{y}) ink={ink}",
                            font.size()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn drawn_rows_match_the_oracle_at_four_bytes_per_pixel() {
        check(SIZE_16, 4);
    }

    #[test]
    fn drawn_rows_match_the_oracle_at_three_bytes_per_pixel() {
        check(SIZE_16, 3);
    }

    #[test]
    fn drawn_rows_match_the_oracle_at_every_font_size() {
        for size in Font::SIZES {
            check(Font::new(size), 4);
        }
    }

    #[test]
    fn the_fourth_byte_is_the_displays() {
        let layout = layout(SIZE_16, 4, 2, 0, 1, 0, 0);
        let mut frame = vec![0; layout.bytes()];
        draw_row(&mut frame, &layout, 0, &cells(b"W "), Some(1));
        assert!(frame.chunks_exact(4).all(|pixel| pixel[3] == 0xff));
        let pixel = &frame[..4];
        assert!(!layout.pixel_is(
            &[pixel[0], pixel[1], pixel[2], 0],
            expected_colour(SIZE_16, &cells(b"W "), Some(1), 0, 0)
        ));
    }

    #[test]
    fn a_cleared_screen_is_background_in_its_margins_too() {
        let layout = layout(SIZE_16, 4, 3, 5, 2, 7, 12);
        let mut frame = vec![0xaa; layout.bytes()];
        clear(&mut frame, &layout);
        for y in 0..layout.height {
            assert!(
                frame[layout.line(y)]
                    .chunks_exact(4)
                    .all(|pixel| layout.pixel_is(pixel, PALETTE[0])),
                "y {y}"
            );
            assert!(frame[layout.line(y).end..(y + 1) * layout.stride_bytes]
                .iter()
                .all(|&byte| byte == 0xaa));
        }
    }

    #[test]
    fn the_oracle_tells_glyphs_apart() {
        let font = SIZE_16;
        let lit = |byte: u8| -> Vec<[u8; 3]> {
            (0..font.glyph_height())
                .flat_map(|y| {
                    (0..font.glyph_width())
                        .map(move |x| expected_colour(font, &cells(&[byte]), None, x, y))
                })
                .collect()
        };
        assert!(lit(b' ').iter().all(|&i| i == PALETTE[0]));
        assert!(lit(b'A').iter().filter(|&&i| i[0] > 110).count() > 10);
        assert_ne!(lit(b'4'), lit(b'2'));
        // The cursor is in the two rows under the glyph, in its column only.
        let (width, height) = (font.glyph_width(), font.glyph_height());
        assert_eq!(
            expected_colour(font, &cells(b"ab"), Some(1), width, height),
            PALETTE[7]
        );
        assert_eq!(
            expected_colour(font, &cells(b"ab"), Some(1), 0, height + 1),
            PALETTE[0]
        );
        assert_eq!(
            expected_colour(font, &cells(b"ab"), Some(1), width * 2, 0),
            PALETTE[0]
        );
    }

    #[test]
    fn a_partial_last_band_is_clipped() {
        let line_height = SIZE_16.line_height();
        let layout = layout(SIZE_16, 4, 4, 0, 1, line_height / 2, 0);
        let mut frame = vec![0; layout.bytes()];
        draw_row(&mut frame, &layout, 1, &cells(b"WWWW"), Some(0));
        assert_eq!((layout.rows(), layout.band(1).len()), (1, line_height / 2));
        assert!(frame.iter().any(|&byte| byte != 0));
    }

    /// The size rule against `docs/console/contract.md` section 2's table.
    #[test]
    fn the_size_rule_picks_the_contracts_font_and_grid() {
        let cases = [
            ((1024, 768), 16, (7, 18), (146, 42)),
            ((1280, 800), 20, (9, 22), (142, 36)),
            ((1280, 960), 20, (9, 22), (142, 43)),
            ((1920, 1080), 24, (11, 26), (174, 41)),
            ((1920, 1200), 24, (11, 26), (174, 46)),
            ((2560, 1600), 32, (14, 34), (182, 47)),
        ];
        for ((width, height), size, cell, grid) in cases {
            let font = Font::for_display(width, height);
            let layout = Layout {
                width,
                height,
                bytes_per_pixel: 4,
                stride_bytes: width * 4,
                reserved: 0,
                bgr: false,
                font,
            };
            assert_eq!(
                (
                    font.size(),
                    (font.glyph_width(), font.line_height()),
                    (layout.cols(), layout.rows())
                ),
                (size, cell, grid),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn a_small_display_uses_size_16_and_a_large_one_is_capped() {
        let small = Font::for_display(800, 600);
        assert_eq!(small.size(), 16);
        let font = Font::for_display(7680, 4320);
        let layout = Layout {
            width: 7680,
            height: 4320,
            bytes_per_pixel: 4,
            stride_bytes: 7680 * 4,
            reserved: 0,
            bgr: false,
            font,
        };
        assert_eq!((font.size(), layout.cols(), layout.rows()), (32, 255, 127));
    }
}

mod pagewalk_tests {
    use super::pagewalk::*;
    use std::collections::HashMap;

    const NX: u64 = 1 << 63;
    const RW: u64 = PRESENT | WRITABLE;

    #[derive(Default)]
    struct Fake {
        tables: HashMap<u64, Box<[u64; 512]>>,
        next: u64,
    }

    impl Fake {
        fn new() -> Self {
            Fake { tables: HashMap::new(), next: 0x10_0000 }
        }
        fn table(&mut self) -> u64 {
            let address = self.next;
            self.next += 0x1000;
            self.tables.insert(address, Box::new([0; 512]));
            address
        }
        fn entry(&self, table: u64, index: usize) -> u64 {
            self.tables[&table][index]
        }
    }

    impl Tables for Fake {
        fn read(&self, table: u64, index: usize) -> u64 {
            self.tables[&table][index]
        }
        fn write(&mut self, table: u64, index: usize, entry: u64) {
            self.tables.get_mut(&table).unwrap()[index] = entry;
        }
        fn allocate(&mut self) -> Option<u64> {
            Some(self.table())
        }
    }

    const APERTURE: u64 = 0x9000_0000;
    const APERTURE_END: u64 = 0x9100_0000;

    /// A 1 GiB page holding the aperture and other device memory, and a 4 KiB
    /// mapping of the aperture's first 3 MiB elsewhere.
    fn tree() -> (Fake, u64, u64, u64) {
        let mut fake = Fake::new();
        let root = fake.table();
        let direct = fake.table();
        fake.write(root, 4, direct | RW);
        fake.write(direct, 2, 0x8000_0000 | RW | PAGE_SIZE | NX);
        let pdpt = fake.table();
        let pd = fake.table();
        let pt = fake.table();
        fake.write(root, 300, pdpt | RW);
        fake.write(pdpt, 0, pd | RW);
        for index in 0..2 {
            let pt = if index == 0 { pt } else { fake.table() };
            fake.write(pd, index, pt | RW);
            for page in 0..512 {
                let address = APERTURE + ((index * 512 + page) as u64) * 0x1000;
                let pat = if page == 7 { PAT_4K } else { 0 };
                fake.write(pt, page, address | RW | PCD | pat);
            }
        }
        (fake, root, direct, pt)
    }

    #[test]
    fn every_mapping_of_the_range_selects_write_combining() {
        let (mut fake, root, direct, pt) = tree();
        let report = set_write_combining(&mut fake, root, APERTURE, APERTURE_END).unwrap();
        assert_eq!(report, Report { pages_4k: 1024, pages_2m: 8, pages_1g: 0, splits: 1, mappings: 2 });
        // The 4 KiB mapping: PWT set, PCD and PAT clear, address kept.
        for page in 0..512 {
            let entry = fake.entry(pt, page);
            assert_eq!(entry & (PWT | PCD | PAT_4K), PWT);
            assert_eq!(entry & 0x000f_ffff_ffff_f000, APERTURE + page as u64 * 0x1000);
        }
        // The 1 GiB page became 2 MiB pages: write-combining over the range,
        // as they were beside it.
        let split = fake.entry(direct, 2);
        assert_eq!(split & PAGE_SIZE, 0);
        let pd = split & 0x000f_ffff_ffff_f000;
        for index in 0..512 {
            let entry = fake.entry(pd, index);
            let address = 0x8000_0000 + index as u64 * 0x20_0000;
            assert_eq!(entry & 0x000f_ffff_ffe0_0000, address);
            assert_eq!(entry & (PAGE_SIZE | NX | RW), PAGE_SIZE | NX | RW);
            let inside = (APERTURE..APERTURE_END).contains(&address);
            assert_eq!(entry & PWT != 0, inside, "entry {index}");
        }
    }

    #[test]
    fn a_large_page_split_keeps_its_memory_type_beside_the_range() {
        let mut fake = Fake::new();
        let root = fake.table();
        let pdpt = fake.table();
        let pd = fake.table();
        fake.write(root, 0, pdpt | RW);
        fake.write(pdpt, 0, pd | RW);
        // PAT entry 4 (PAT bit, PCD and PWT clear), in a 2 MiB page.
        fake.write(pd, 3, 0x60_0000 | RW | PAGE_SIZE | PAT_LARGE);
        let report = set_write_combining(&mut fake, root, 0x68_0000, 0x6c_0000).unwrap();
        assert_eq!(report, Report { pages_4k: 64, pages_2m: 0, pages_1g: 0, splits: 1, mappings: 1 });
        let pt = fake.entry(pd, 3) & 0x000f_ffff_ffff_f000;
        for index in 0..512 {
            let entry = fake.entry(pt, index);
            let address = 0x60_0000 + index as u64 * 0x1000;
            assert_eq!(entry & 0x000f_ffff_ffff_f000, address);
            let flags = entry & (PWT | PCD | PAT_4K);
            if (0x68_0000..0x6c_0000).contains(&address) {
                assert_eq!(flags, PWT);
            } else {
                assert_eq!(flags, PAT_4K, "entry {index} keeps PAT entry 4");
            }
        }
    }

    #[test]
    fn mappings_elsewhere_are_untouched() {
        let mut fake = Fake::new();
        let root = fake.table();
        let pdpt = fake.table();
        fake.write(root, 0, pdpt | RW);
        fake.write(pdpt, 0, RW | PAGE_SIZE | PCD | PWT);
        let report = set_write_combining(&mut fake, root, APERTURE, APERTURE_END).unwrap();
        assert_eq!(report, Report::default());
        assert_eq!(fake.entry(pdpt, 0), RW | PAGE_SIZE | PCD | PWT);
    }
}

extern crate alloc;
include!("vt_support.rs");

#[path = "../../../kernel/src/tty/keyboard_input.rs"]
mod keyboard_input;
#[path = "../../../kernel/src/console/keymap.rs"]
pub mod keymap;
#[path = "../../../kernel/src/tty/input_source.rs"]
pub mod input_source;
use input_source::InputSource;

#[test]
fn deferred_keys_stay_ordered_through_discipline_contention() {
    let mut keys = keyboard_input::PendingInput::new();
    let bytes = b"ab\x7f";
    for &byte in bytes {
        assert!(keys.push(byte, InputSource::Other));
    }
    keys.drain(|_, _| false);
    let mut received = Vec::new();
    keys.drain(|byte, _| {
        received.push(byte);
        true
    });
    assert_eq!(received, bytes);
    let mut discipline = line_discipline::LineDiscipline::new();
    let mut signals = Vec::new();
    for byte in received {
        if let Some(signal) = discipline.input_char(byte, &mut |_| {}) {
            signals.push(signal);
        }
        assert!(
            !discipline.has_data(),
            "canonical input escaped before newline"
        );
    }
    assert!(signals.is_empty());
    discipline.input_char(b'\n', &mut |_| {});
    let mut edited = [0; 8];
    assert_eq!(discipline.read(&mut edited), Ok(2));
    assert_eq!(&edited[..2], b"a\n");
    for &byte in b"x\x03cd" {
        assert!(keys.push(byte, InputSource::Other));
    }
    keys.drain(|byte, _| {
        if let Some(signal) = discipline.input_char(byte, &mut |_| {}) {
            signals.push(signal);
        }
        true
    });
    assert_eq!(signals, [2]);
    assert!(!discipline.has_data());
    discipline.input_char(b'\n', &mut |_| {});
    let mut line = [0; 8];
    assert_eq!(discipline.read(&mut line), Ok(3));
    assert_eq!(&line[..3], b"cd\n");
    keys.drain(|_, _| panic!("processed twice"));
}

#[test]
fn scrollback_view_stays_on_the_same_row_after_the_ring_fills() {
    let mut g = grid::Grid::new();
    let cells = vec![grid::BLANK; 16 * 3].leak();
    let alternate = vec![grid::BLANK; 16 * 3].leak();
    g.set_storage(cells, alternate);
    g.set_size(16, 3);
    g.set_history(vec![grid::BLANK; 16 * grid::HISTORY_LINES].leak());
    for n in 0..1100 {
        g.write(format!("{n}\r\n").as_bytes());
    }
    g.scroll_view(10);
    let viewed = g.view_row(0).to_vec();
    g.write(b"1100\r\n");
    assert_eq!(g.view_row(0), viewed);
    assert_eq!(g.view(), 11);
    g.scroll_view(2000);
    g.write(b"1101\r\n");
    assert_eq!(g.view(), grid::HISTORY_LINES);
}

#[path = "../../../kernel/src/tty/line_discipline.rs"]
mod line_discipline;
#[path = "../../../kernel/src/tty/termios.rs"]
mod termios;

/// The kernel's `tty::read_to_user` refuses address 0 before taking any byte;
/// copying to a real user address is outside the host discipline tests.
pub fn read_to_user(
    ptr: u64,
    n: usize,
    _: impl FnMut(&mut [u8]) -> usize,
) -> (usize, Result<usize, i32>) {
    if n > 0 && ptr == 0 {
        return (0, Err(14));
    }
    panic!("user address copying is outside the host discipline tests")
}

#[test]
fn canonical_overflow_rings_and_keeps_editing_and_delimiter() {
    let mut d = line_discipline::LineDiscipline::new();
    let mut echoed = Vec::new();
    for _ in 0..4095 { d.input_char(b'x', &mut |c| echoed.push(c)); }
    echoed.clear();
    d.input_char(b'y', &mut |c| echoed.push(c));
    assert_eq!(echoed, [7], "a discarded byte must have an audible response");
    d.input_char(0x7f, &mut |_| {});
    d.input_char(b'z', &mut |_| {});
    d.input_char(b'\n', &mut |_| {});
    let mut line = [0; 4096];
    assert_eq!(d.read(&mut line), Ok(4096));
    assert!(line[..4094].iter().all(|&c| c == b'x'));
    assert_eq!(&line[4094..], b"z\n");
}
