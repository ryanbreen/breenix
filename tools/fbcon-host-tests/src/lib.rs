//! Host tests for the x86 screen console's grid, its drawing against the
//! read-back oracle, and the write-combining page-table walk.
#![cfg(test)]

#[path = "../../../kernel/src/fbcon/grid.rs"]
mod grid;
#[path = "../../../kernel/src/fbcon/pagewalk.rs"]
mod pagewalk;
#[path = "../../../kernel/src/fbcon/raster.rs"]
mod raster;

mod grid_tests {
    use super::grid::Grid;

    fn grid(cols: usize, rows: usize) -> Box<Grid> {
        let mut grid = Box::new(Grid::new());
        grid.set_size(cols, rows);
        grid
    }

    fn text(grid: &Grid, row: usize) -> String {
        String::from_utf8(grid.row(row).to_vec()).unwrap().trim_end().to_string()
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
        assert_eq!([text(&g, 0), text(&g, 1), text(&g, 2)], ["login:", "WARN: x", "root"]);
    }

    #[test]
    fn a_utf8_character_takes_one_cell() {
        let mut g = grid(10, 2);
        g.write("é!".as_bytes());
        assert_eq!((text(&g, 0), g.cursor()), ("?!".into(), (0, 2)));
    }

    #[test]
    fn a_grid_larger_than_its_storage_is_cut_to_it() {
        let mut g = grid(10_000, 10_000);
        assert_eq!((g.cols(), g.rows()), (super::grid::MAX_COLS, super::grid::MAX_ROWS));
        g.write(&vec![b'x'; super::grid::MAX_COLS * super::grid::MAX_ROWS + 1]);
        assert_eq!(text(&g, super::grid::MAX_ROWS - 1), "x");
    }

    #[test]
    fn every_write_changes_the_generation() {
        let mut g = grid(10, 2);
        let before = g.generation();
        g.write(b"x");
        assert_ne!(g.generation(), before);
    }
}

mod raster_tests {
    use super::raster::{
        draw_row, expected_intensity, pixel_is, Layout, CURSOR, GLYPH_HEIGHT, GLYPH_WIDTH, LINE_HEIGHT,
    };

    /// A framebuffer `cols` cells and `extra` pixels wide, `rows` text rows and
    /// `extra_lines` pixel rows tall, with `pad` bytes after each pixel row.
    fn layout(bpp: usize, cols: usize, extra: usize, rows: usize, extra_lines: usize, pad: usize) -> Layout {
        let width = cols * GLYPH_WIDTH + extra;
        Layout { width, height: rows * LINE_HEIGHT + extra_lines, bytes_per_pixel: bpp, stride_bytes: width * bpp + pad }
    }

    fn printable() -> Vec<u8> {
        (0x20..0x7f).collect()
    }

    /// Each pixel of each band, against the oracle; padding is left alone.
    fn check(bpp: usize) {
        let ascii = printable();
        let layout = layout(bpp, ascii.len(), 5, 3, 7, 12);
        let mut frame = vec![0xaa; layout.bytes()];
        let rows: [(&[u8], Option<usize>); 3] =
            [(&ascii, Some(10)), (b"metal-login-ok 42", None), (b"#\x7f\x01", Some(2))];
        for (row, (text, cursor)) in rows.iter().enumerate() {
            let mut cells = text.to_vec();
            cells.resize(layout.cols(), b' ');
            draw_row(&mut frame, &layout, row, &cells, *cursor);
        }
        for (row, (text, cursor)) in rows.iter().enumerate() {
            let mut cells = text.to_vec();
            cells.resize(layout.cols(), b' ');
            for y in layout.band(row) {
                let line = &frame[layout.line(y)];
                for (x, pixel) in line.chunks_exact(bpp).enumerate() {
                    let expected = expected_intensity(&cells, *cursor, x, y - layout.band(row).start);
                    assert!(pixel_is(pixel, expected), "row {row} x {x} y {y}: {pixel:?} vs {expected}");
                }
                let padding = layout.line(y).end..(y + 1) * layout.stride_bytes;
                assert!(frame[padding].iter().all(|&byte| byte == 0xaa));
            }
        }
    }

    #[test]
    fn drawn_rows_match_the_oracle_at_four_bytes_per_pixel() {
        check(4);
    }

    #[test]
    fn drawn_rows_match_the_oracle_at_three_bytes_per_pixel() {
        check(3);
    }

    #[test]
    fn the_oracle_tells_glyphs_apart() {
        let lit = |byte: u8| -> Vec<u8> {
            (0..GLYPH_HEIGHT)
                .flat_map(|y| (0..GLYPH_WIDTH).map(move |x| expected_intensity(&[byte], None, x, y)))
                .collect()
        };
        assert!(lit(b' ').iter().all(|&i| i == 0));
        assert!(lit(b'A').iter().filter(|&&i| i > 0x80).count() > 10);
        assert_ne!(lit(b'4'), lit(b'2'));
        // The cursor is in the two rows under the glyph, in its column only.
        assert_eq!(expected_intensity(b"ab", Some(1), GLYPH_WIDTH, GLYPH_HEIGHT), CURSOR);
        assert_eq!(expected_intensity(b"ab", Some(1), 0, GLYPH_HEIGHT + 1), 0);
        assert_eq!(expected_intensity(b"ab", Some(1), GLYPH_WIDTH * 2, 0), 0);
    }

    #[test]
    fn a_partial_last_band_is_clipped() {
        let layout = layout(4, 4, 0, 1, LINE_HEIGHT / 2, 0);
        let mut frame = vec![0; layout.bytes()];
        draw_row(&mut frame, &layout, 1, b"WWWW", Some(0));
        assert_eq!((layout.rows(), layout.band(1).len()), (1, LINE_HEIGHT / 2));
        assert!(frame.iter().any(|&byte| byte != 0));
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
