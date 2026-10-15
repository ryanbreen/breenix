//! The window interface cases: windows, isolation and events, and the window cases of
//! composition, scanout and panel.

mod windows;
mod isolation;
mod composition;
mod events;
mod scanout;
mod panel;

pub(crate) use windows::{WINDOWS};
pub(crate) use isolation::{ISOLATION};
pub(crate) use composition::{COMPOSITION, COMPOSITION_FLUSH};
pub(crate) use events::{EVENTS};
pub(crate) use scanout::{SCANOUT};
pub(crate) use panel::{PANEL};
