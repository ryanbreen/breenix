//! libbui — Breenix UI Widget Toolkit
//!
//! Lightweight retained-mode widget library for Breenix graphical applications.
//! Widgets are standalone structs with `update()` and `draw()` methods.
//! Drawing logic uses `FrameBuf`; semantic nodes share the BreenGL tree codec.

#![no_std]

extern crate alloc;

pub mod focus;
pub mod input;
pub mod layout;
pub mod rect;
pub mod shortcut;
pub mod text;
pub mod theme;
pub mod tree;
pub mod widget;

pub use input::{InputState, Key, KeyInput, WidgetEvent};
pub use rect::Rect;
pub use theme::Theme;

mod drawing;
