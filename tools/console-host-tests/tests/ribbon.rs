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
            let rows = ribbon::Ribbon::text_rows(&layout);
            assert_eq!(rows, 752 * scale / layout.font.line_height());
            assert!(layout.band(rows - 1).end <= 752 * scale);
            assert!(rows < layout.rows());
        }
    }

}
