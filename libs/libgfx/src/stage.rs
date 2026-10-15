//! Deterministic run showcase, BreenGL contract section 23.
//! Inputs are immutable result snapshots. The caller owns the UI clock and display handoff.
use crate::{color::Color, framebuf::FrameBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp { Pending, Running, Pass, Fail, NotReached }
#[derive(Clone, Copy)]
pub struct Image<'a> { pub pixels: &'a [u8], pub width: usize, pub height: usize }
#[derive(Clone, Copy)]
pub struct Tile<'a> {
    pub id: &'a str, pub stamp: Stamp, pub key: u64, pub thumbnail: Option<Image<'a>>,
}
#[derive(Clone, Copy)]
pub struct Milestone<'a> { pub id: &'a str, pub first: usize, pub end: usize }
#[derive(Clone, Copy)]
pub enum Spotlight<'a> {
    None,
    Picture { expected: Image<'a>, actual: Image<'a>, first: Option<(usize,usize)>, differing: usize, bound: usize },
    Timing { frame_us: &'a [u32], budget_us: u32 },
}
#[derive(Clone, Copy)]
pub struct Finale<'a> { pub started_ms: u64, pub slowest: &'a str, pub slowest_ms: u64 }
impl Finale<'_> { pub fn hold_complete(&self, ui_time_ms:u64) -> bool { ui_time_ms.saturating_sub(self.started_ms) >= 5000 } }
pub struct Run<'a> {
    pub suite: &'a str, pub target: &'a str, pub case_number: usize, pub ui_time_ms: u64, pub fps: u32,
    pub tiles: &'a [Tile<'a>], pub milestones: &'a [Milestone<'a>], pub selected: usize,
    pub spotlight: Spotlight<'a>, pub finale: Option<Finale<'a>>, pub boundary: &'a str,
}
#[derive(Debug, PartialEq, Eq)]
pub enum StageError { Size, Manifest, Image, Timing, MissingStage }

pub fn draw(fb: &mut FrameBuf, run: &Run<'_>) -> Result<(),StageError> {
    // The independent fixtures land before the renderer.
    let _ = (fb,run,Color::BLACK);
    Err(StageError::MissingStage)
}
