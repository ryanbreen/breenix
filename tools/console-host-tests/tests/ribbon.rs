extern crate alloc;
#[path = "../../../kernel/src/console/boxdraw.rs"]
mod boxdraw;
#[path = "../../../kernel/src/console/grid.rs"]
pub mod grid;
#[path = "../../../kernel/src/console/raster.rs"]
pub mod raster;

#[path = "../../../kernel/src/graphics/boot_ribbon.rs"]
pub mod boot_ribbon;
#[path = "../../../kernel/src/graphics/boot_producer.rs"]
pub mod boot_producer;
pub mod graphics {
    pub use crate::{boot_producer, boot_ribbon};
}
pub fn arch_interrupts_enabled() -> bool { true }
#[path = "../../../kernel/src/console/ribbon.rs"]
pub mod ribbon;

mod ribbon_tests {
    use super::*;

    #[test]
    fn ribbon_band_reserves_whole_text_rows_at_both_scales() {
        for scale in [1, 2] {
            let layout = raster::Layout {
                width: 1280 * scale, height: 800 * scale,
                bytes_per_pixel: 4, stride_bytes: 1280 * scale * 4,
                reserved: 255, bgr: true,
                font: raster::Font::for_display(1280 * scale, 800 * scale),
            };
            let rows = ribbon::Ribbon::text_rows(&layout, true);
            assert_eq!(rows, 752 * scale / layout.font.line_height());
            assert!(layout.band(rows - 1).end <= 752 * scale);
            assert_eq!(ribbon::Ribbon::text_rows(&layout, false), layout.rows());
            assert!(rows < layout.rows());
        }
    }

    #[test]
    fn reclaim_rows_preserves_scrolled_text_history_and_alternate_screen() {
        let mut g = grid::Grid::new();
        g.set_storage(vec![grid::BLANK; 8 * 6].leak(), vec![grid::BLANK; 8 * 6].leak());
        g.set_size(8, 4);
        g.set_history(vec![grid::BLANK; grid::HISTORY_LINES * 8].leak());
        g.write(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let before: Vec<_> = (0..4).map(|r| g.row(r).to_vec()).collect();
        let cursor = g.cursor();
        g.write(b"\x1b[?1049hALT");
        let alternate = g.row(0).to_vec();
        g.grow_rows(6);
        assert_eq!(g.rows(), 6);
        assert_eq!(g.row(0), alternate);
        assert!(g.row(5).iter().all(|&c| c == grid::BLANK));
        g.write(b"\x1b[?1049l");
        assert_eq!(g.cursor(), cursor);
        for (r, text) in before.iter().enumerate() { assert_eq!(g.row(r), text); }
        assert!(g.row(4).iter().all(|&c| c == grid::BLANK));
        assert!(g.scroll_view(1));
        assert_eq!(g.view_row(0)[0].character(), 'o');
        g.scroll_view(-1);
        g.write(b"\x1b[6;1Hbottom");
        assert_eq!(g.row(5)[0].character(), 'b');
    }
}
