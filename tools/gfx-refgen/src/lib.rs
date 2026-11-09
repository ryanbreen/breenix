//! Host widget reference rendering: production libbui geometry and text, CPU raster.
//! The aliases bind included widget sources to the reference surface in this crate;
//! production libbui continues to use GPU surfaces.
extern crate alloc;
extern crate self as libbui;
extern crate self as libgfx;
pub use libgfx_ref::{bitmap_font, color, font, shapes, ttf_font};

pub mod surface {
    pub use libgfx_ref::framebuf::Rect;
    use libgfx_ref::framebuf::{Format, FrameBuf};
    /// A CPU reference target with the same logical scale as the guest surface.
    pub struct Surface {
        frame: FrameBuf,
        scale: u32,
    }
    impl Surface {
        pub fn new(scale: u32) -> Self {
            assert!((1..=2).contains(&scale));
            Self {
                frame: FrameBuf::new(640 * scale as usize, 400 * scale as usize, Format::BGRX)
                    .unwrap(),
                scale,
            }
        }
        pub fn ui_scale(&self) -> u32 {
            self.scale
        }
    }
    impl std::ops::Deref for Surface {
        type Target = FrameBuf;
        fn deref(&self) -> &FrameBuf {
            &self.frame
        }
    }
    impl std::ops::DerefMut for Surface {
        fn deref_mut(&mut self) -> &mut FrameBuf {
            &mut self.frame
        }
    }
}
#[path = "../../../libs/libbui/src/drawing.rs"]
pub mod drawing;
#[path = "../../../libs/libbui/src/focus.rs"]
pub mod focus;
#[path = "../../../libs/libbui/src/input.rs"]
pub mod input;
#[path = "../../../libs/libbui/src/layout.rs"]
pub mod layout;
#[path = "../../../libs/libbui/src/rect.rs"]
pub mod rect;
#[path = "../../../libs/libbui/src/shortcut.rs"]
pub mod shortcut;
#[path = "../../../libs/libbui/src/text.rs"]
pub mod text;
#[path = "../../../libs/libbui/src/theme.rs"]
pub mod theme;
#[path = "../../../libs/libbui/src/tree.rs"]
pub mod tree;
#[path = "../../../libs/libbui/src/widget/mod.rs"]
pub mod widget;
pub use input::{InputState, Key, KeyInput, WidgetEvent};
pub use rect::Rect;
pub use theme::Theme;
#[path = "../../../userspace/programs/src/uitest_suite/proof_app_tree.rs"]
pub mod app_tree;
#[path = "../../../userspace/programs/src/uitest_widget_layout.rs"]
pub mod uitest_widget_layout;
pub mod widget_references;
#[path = "../../../userspace/programs/src/uitest_widgets.rs"]
pub mod widgets;

#[cfg(test)]
#[path = "../../../tests/ui_wm_scenarios.rs"]
mod ui_wm_scenarios;
