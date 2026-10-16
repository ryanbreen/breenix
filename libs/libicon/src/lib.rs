//! Breenix Animated Icon Library
//!
//! Physics-based interactive icons that react to mouse hover and clicks
//! with spring animations, particles, and procedural effects.

#![no_std]
extern crate alloc;

pub mod easing;
pub mod icon;
pub mod particles;
pub mod physics;
pub mod icons;

/// The reviewed catalogue scene: 640×512, four icon rows and five state columns.
/// Columns are Idle, HoverIn, Hovering, Pressed, and Clicked; §16 specifies
/// each input schedule so gfx-refgen and the guest render the same scene.
pub fn draw_catalogue(fb: &mut libgfx::framebuf::FrameBuf) {
    use alloc::boxed::Box;
    use icon::{Icon, IconMouse};
    for row in 0..4 {
        for col in 0..5 {
            let mut icon: Box<dyn Icon> = match row {
                0 => Box::new(icons::home::HomeIcon::new()),
                1 => Box::new(icons::search::SearchIcon::new()),
                2 => Box::new(icons::save::SaveIcon::new()),
                _ => Box::new(icons::arrow::ArrowIcon::new(true)),
            };
            let mut input = IconMouse::none(); input.hovering = true;
            if col == 1 { icon.update(101_000,input); }
            if col >= 2 { icon.update(251_000,input); }
            if col >= 3 {
                input.pressed = true; input.just_clicked = true;
                icon.update(1_000,input); input.just_clicked = false;
                icon.update(50_000,input);
            }
            if col == 4 {
                input.pressed = false; input.just_released = true;
                icon.update(1_000,input);input.just_released = false;
                icon.update(120_000,input);
            }
            icon.draw(fb,64+col*128,64+row*128,48);
        }
    }
}
