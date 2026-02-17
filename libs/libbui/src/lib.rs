//! libbui — Breenix UI Widget Toolkit
//!
//! Lightweight retained-mode widget library for Breenix graphical applications.
//! Widgets are standalone structs with `update()` and `draw()` methods.
//! No libbreenix dependency — pure drawing logic on `FrameBuf`.

#![no_std]

extern crate alloc;

pub mod input;
pub mod layout;
pub mod rect;
pub mod text;
pub mod theme;
pub mod widget;

pub use input::{InputState, WidgetEvent};
pub use rect::Rect;
pub use theme::Theme;
