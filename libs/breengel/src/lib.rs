//! Breengel — Breenix Graphical Environment Library
//!
//! Client library for GUI applications on Breenix. Apps create windows via
//! Breengel, render into pixel buffers, and receive input events from BWM.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────┐  ┌──────────┐  ┌──────────┐
//! │  bterm   │  │  bcheck  │  │  bounce  │   ← GUI apps (Breengel clients)
//! └────┬─────┘  └────┬─────┘  └────┬─────┘
//!      │             │             │
//!      └─────────────┼─────────────┘
//!                    │
//!              ┌─────┴─────┐
//!              │  Breengel │   ← This library
//!              └─────┬─────┘
//!                    │
//!         ┌──────────┼──────────┐
//!         │          │          │
//!    ┌────┴────┐ ┌───┴───┐ ┌───┴───┐
//!    │libbreenix│ │libgfx │ │libbui │
//!    │(syscalls)│ │(draw) │ │(widg.)│
//!    └─────────┘ └───────┘ └───────┘
//! ```
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use breengel::{Window, Event};
//!
//! let mut win = Window::new("My App", 400, 300).unwrap();
//! loop {
//!     // Poll for input events
//!     for event in win.poll_events() {
//!         match event {
//!             Event::KeyPress { ascii, .. } => { /* handle key */ }
//!             Event::CloseRequested => std::process::exit(0),
//!             _ => {}
//!         }
//!     }
//!     // Draw into the pixel buffer
//!     let fb = win.framebuf();
//!     // ... render with libgfx ...
//!     win.present().unwrap();
//! }
//! ```

mod window;
mod event;

pub use window::Window;
pub use event::{Event, Modifiers};
pub use libbreenix::graphics::{WindowInputEvent, input_event_type};
pub use libgfx::framebuf::FrameBuf;
pub use libgfx::color::Color;
pub use libbui::widget::tab_bar::TabBar;
pub use libbui::{InputState, Rect, Theme};
