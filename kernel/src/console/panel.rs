//! Retained suite status panel; only its reserved rows are published on a change.
use alloc::vec::Vec;
use core::ops::Range;
use noto_sans_mono_bitmap::RasterHeight;
use super::{raster::{self, Font, Layout}, ribbon::Ribbon, status::{self, PanelGeometry, PanelMode, Rect, SuiteStatus}};

#[derive(Clone, PartialEq, Eq)]
pub struct Panel {
    geometry: PanelGeometry,
    suite: SuiteStatus,
    heading: Vec<char>,
    mode: PanelMode,
    ribbon: Option<Ribbon>,
}

impl Panel {
    pub fn new(layout: &Layout, suite: SuiteStatus, reached: Vec<bool>, drop_stage: bool, now_ms: u64) -> Option<Self> {
        let geometry = layout.panel()?;
        let mode = suite.mode(!reached.is_empty() && reached.iter().all(|&v| v));
        let heading = suite.heading(now_ms).unwrap_or_default().chars().collect();
        let ribbon = Ribbon::new(layout, reached, drop_stage);
        Some(Self { geometry, suite, heading, mode, ribbon })
    }

    pub fn rows(&self) -> Range<usize> {
        self.geometry.panel.y..self.geometry.panel.y + self.geometry.panel.height
    }

    fn fonts(&self, layout: &Layout) -> (Font, Font, usize) {
        let scale = self.geometry.stage_scale;
        if scale == 0 { (layout.font, layout.font, 1) }
        else { (Font::new(RasterHeight::Size20), Font::new(RasterHeight::Size16), scale) }
    }

    pub fn paint(&self, frame: &mut [u8], layout: &Layout) {
        fill(frame, layout, self.geometry.panel, status::BACKGROUND);
        if self.mode == PanelMode::Empty { return; }
        let (heading, label, scale) = self.fonts(layout);
        if self.mode == PanelMode::Suite {
            raster::panel_text(frame, layout, self.geometry.heading, heading, scale, &self.heading);
            let r = self.geometry.track;
            fill(frame, layout, r, status::PENDING);
            if self.suite.total() > 0 {
                let width = r.width / scale;
                for (i, failed) in self.suite.judged().enumerate() {
                    let a = width * i / self.suite.total() * scale;
                    let b = width * (i + 1) / self.suite.total() * scale;
                    fill(frame, layout, Rect::new(r.x + a, r.y, b - a, r.height),
                        if failed { status::FAIL } else { status::PASS });
                }
            }
        }
        raster::panel_text(frame, layout, self.geometry.label, label, scale, &LABEL);
        if let Some(ribbon) = &self.ribbon { ribbon.paint(frame, layout, ribbon.rows()); }
    }

    /// Derive the expected pixel from retained state, font samples and the colour model.
    pub fn colour(&self, layout: &Layout, x: usize, y: usize) -> Option<[u8; 3]> {
        let g = self.geometry;
        if !g.panel.contains(x,y) { return None; }
        if self.mode == PanelMode::Empty { return Some(status::BACKGROUND); }
        let (heading, label, scale) = self.fonts(layout);
        if self.mode == PanelMode::Suite {
            if g.heading.contains(x,y) {
                return Some(raster::panel_text_colour(heading, scale, &self.heading, x-g.heading.x, y-g.heading.y));
            }
            if let Some(rgb) = g.progress_colour(&self.suite,x,y) { return Some(rgb); }
        }
        if g.label.contains(x,y) {
            return Some(raster::panel_text_colour(label, scale, &LABEL, x-g.label.x,y-g.label.y));
        }
        Some(self.ribbon.as_ref().and_then(|r| r.colour(x,y)).unwrap_or(status::BACKGROUND))
    }
}

const LABEL: [char; 11] = ['B','o','o','t',' ','s','t','a','g','e','s'];

fn fill(frame: &mut [u8], layout: &Layout, rect: Rect, colour: [u8; 3]) {
    for y in rect.y..(rect.y+rect.height).min(layout.height) {
        let line = layout.line(y).start;
        let a = line + rect.x.min(layout.width)*layout.bytes_per_pixel;
        let b = line + (rect.x+rect.width).min(layout.width)*layout.bytes_per_pixel;
        layout.fill(&mut frame[a..b],colour);
    }
}
