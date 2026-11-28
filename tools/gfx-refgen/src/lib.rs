//! Host reference rendering for widgets and icons on the CPU raster.
//! The aliases bind the included libbui and libicon sources to the reference surface.
extern crate alloc;
#[cfg(test)]
extern crate self as breengel;
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
        pub fn from_frame(frame: FrameBuf) -> Self {
            Self { frame, scale: 1 }
        }
        pub fn into_frame(self) -> FrameBuf {
            self.frame
        }
        pub fn width(&self) -> usize {
            self.frame.width
        }
        pub fn height(&self) -> usize {
            self.frame.height
        }
        pub fn batch(&mut self, draw: impl FnOnce(&mut Self)) {
            // The CPU raster draws each shape immediately.
            draw(self);
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
#[path = "../../../libs/libicon/src/easing.rs"]
pub mod easing;
#[path = "../../../libs/libicon/src/icon.rs"]
pub mod icon;
#[path = "../../../libs/libicon/src/icons/mod.rs"]
pub mod icons;
#[path = "../../../libs/libicon/src/particles.rs"]
pub mod particles;
#[path = "../../../libs/libicon/src/physics.rs"]
pub mod physics;
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
mod ui_wm_scenarios;
