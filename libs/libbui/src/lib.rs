//! libbui — Breenix UI Widget Toolkit
//!
//! Lightweight retained-mode widget library for Breenix graphical applications.
//! Widgets are standalone structs with `update()` and `draw()` methods.
//! Drawing uses GPU surfaces; window-aware text-field keys use the kernel clipboard.

#![no_std]

extern crate alloc;

pub mod focus;
pub mod input;
pub mod layout;
pub mod rect;
pub mod shortcut;
pub mod text;
pub mod theme;
pub mod widget;

pub use input::{InputState, Key, KeyInput, WidgetEvent};
pub use rect::Rect;
pub use theme::Theme;

mod drawing;
