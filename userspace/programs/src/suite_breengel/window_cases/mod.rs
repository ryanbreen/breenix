//! The window interface cases: windows, isolation and events, and the window cases of
//! composition, scanout and panel.

pub(crate) mod kit;
pub(crate) mod handoff;
mod windows;
mod isolation;
pub(crate) mod composition;
mod events;
mod scanout;
mod panel;

pub(crate) use windows::{WINDOWS};
pub(crate) use isolation::{ISOLATION};
pub(crate) use composition::{COMPOSITION};
pub(crate) use events::{EVENTS};
pub(crate) use scanout::{SCANOUT};
pub(crate) use panel::{PANEL};
