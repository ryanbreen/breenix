//! The run stage's painter (ruling B36): libgfx's GPU stage behind libbreenix's interface,
//! drawing each frame the runner's oracle also draws, with its pictures as GPU references.

use libbreenix::suite::{PictureRef, SpotRef, StageFrame, ThumbRef};
use libgfx::blit::Source;
use libgfx::color::Color;
use libgfx::stage as gpu;
use libgfx::surface::{Format, Surface};
use libgfx_ref::stage as oracle;

/// A resource of this process the stage samples.
struct Held {
    handle: u64,
    width: usize,
    height: usize,
}
impl Source for Held {
    fn resource(&self) -> u64 {
        self.handle
    }
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
    fn format(&self) -> Format {
        Format::BGRX
    }
}

fn rgb(c: [u8; 3]) -> Color {
    Color::rgb(c[0], c[1], c[2])
}

fn picture<'a>(p: &PictureRef, h: &'a Held) -> gpu::Picture<'a> {
    gpu::Picture { source: h, x: p.x, y: p.y, width: p.width, height: p.height, cover: p.cover.map(|(r, c)| (r, rgb(c))) }
}

fn stamp(s: oracle::Stamp) -> gpu::Stamp {
    match s {
        oracle::Stamp::Pending => gpu::Stamp::Pending,
        oracle::Stamp::Running => gpu::Stamp::Running,
        oracle::Stamp::Pass => gpu::Stamp::Pass,
        oracle::Stamp::Fail => gpu::Stamp::Fail,
        oracle::Stamp::NotReached => gpu::Stamp::NotReached,
    }
}

fn points(p: &[oracle::InputPoint]) -> Vec<gpu::InputPoint> {
    p.iter().map(|p| gpu::InputPoint { x: p.x, y: p.y, pressed: p.pressed }).collect()
}

/// This process's canvas and the stage's renderer (its font and wall); a forked case makes
/// its own.
pub(crate) struct StagePainter {
    pid: u64,
    canvas: Option<Surface>,
    stage: gpu::Stage,
}

impl StagePainter {
    pub(crate) fn new() -> StagePainter {
        StagePainter { pid: 0, canvas: None, stage: gpu::Stage::new() }
    }
}

impl libbreenix::suite::Painter for StagePainter {
    fn canvas(&mut self, width: u32, height: u32) -> Result<u64, String> {
        let pid = libbreenix::process::getpid().map_err(|e| e.to_string())?.raw() as u64;
        let (w, h) = (width as usize, height as usize);
        if pid != self.pid {
            // The parent's canvas, glyph atlas and wall are not this process's to use, nor to
            // destroy: their handle numbers may name this process's own resources.
            core::mem::forget(self.canvas.take());
            core::mem::forget(core::mem::replace(&mut self.stage, gpu::Stage::new()));
            self.pid = pid;
        }
        if self.canvas.as_ref().is_none_or(|c| (c.width(), c.height()) != (w, h) || c.error().is_some()) {
            self.canvas = None;
            self.canvas = Some(Surface::new(w, h, Format::BGRX).map_err(|e| format!("stage canvas: {e}"))?);
        }
        Ok(self.canvas.as_ref().map_or(0, Surface::handle))
    }

    fn context(&mut self) -> Result<u64, String> {
        gpu::context().map_err(|e| format!("stage painter context: {e:?}"))
    }

    fn declared_bytes(&mut self) -> u64 {
        gpu::declared_bytes().unwrap_or(u64::MAX)
    }

    fn settle(&mut self) -> Result<(), String> {
        let pid = libbreenix::process::getpid().map_err(|e| e.to_string())?.raw() as u64;
        if pid != self.pid {
            return Ok(());
        }
        self.stage.wait().map_err(|e| format!("stage wall: {e}"))?;
        self.canvas.as_ref().map_or(Ok(()), Surface::wait).map_err(|e| format!("stage canvas: {e}"))
    }

    fn retire(&mut self) -> Result<(), String> {
        self.settle()?;
        self.canvas = None;
        self.stage = gpu::Stage::new();
        gpu::retire_context().map_err(|e| format!("stage renderer retirement: {e:?}"))?;
        Ok(())
    }

    fn paint(&mut self, f: &StageFrame<'_>) -> Result<(), String> {
        self.canvas(f.width, f.height)?;
        let canvas = self.canvas.as_mut().ok_or("stage canvas missing")?;
        // The picture references, in the order they are used below.
        let spot_refs: Vec<PictureRef> = match f.spot {
            SpotRef::None => Vec::new(),
            SpotRef::Actual(p) => vec![*p],
            SpotRef::Picture { expected, actual, difference } => [Some(*expected), Some(*actual), difference.artifact].into_iter().flatten().collect(),
        };
        let thumb_refs: Vec<PictureRef> = f.thumbs.iter().filter_map(|t| match t {
            Some(ThumbRef::Picture(p)) => Some(*p),
            _ => None,
        }).collect();
        let held: Vec<Held> = thumb_refs.iter().chain(&spot_refs).map(|p| Held { handle: p.handle, width: p.width, height: p.height }).collect();
        let mut next = 0;
        let tiles: Vec<gpu::Tile<'_>> = f.run.tiles.iter().zip(f.thumbs).map(|(t, thumb)| gpu::Tile {
            id: t.id,
            stamp: stamp(t.stamp),
            key: t.key,
            thumbnail: thumb.as_ref().map(|thumb| match thumb {
                ThumbRef::Picture(p) => {
                    next += 1;
                    gpu::Thumbnail::Picture(picture(p, &held[next - 1]))
                }
                ThumbRef::Solid { width, height, rgb: c } => gpu::Thumbnail::Solid { width: *width, height: *height, colour: rgb(*c) },
            }),
        }).collect();
        let spot_held = &held[thumb_refs.len()..];
        let milestones: Vec<gpu::Milestone<'_>> = f.run.milestones.iter().map(|m| gpu::Milestone { id: m.id, first: m.first, end: m.end }).collect();
        let (spot_points, input_points);
        let heat: Vec<([usize; 4], Color)>;
        let spotlight = match (&f.run.spotlight, f.spot) {
            (oracle::Spotlight::Actual(_), SpotRef::Actual(p)) => gpu::Spotlight::Actual(picture(p, &spot_held[0])),
            (oracle::Spotlight::Picture { differing, bound, .. }, SpotRef::Picture { expected, actual, difference }) => {
                heat = difference.heat.iter().map(|(r, c)| (*r, rgb(*c))).collect();
                gpu::Spotlight::Picture {
                    expected: picture(expected, &spot_held[0]),
                    actual: picture(actual, &spot_held[1]),
                    difference: gpu::Difference {
                        artifact: difference.artifact.as_ref().map(|a| picture(a, &spot_held[2])),
                        heat: &heat,
                        cross: difference.cross,
                    },
                    differing: *differing,
                    bound: *bound,
                }
            }
            (oracle::Spotlight::Actual(_) | oracle::Spotlight::Picture { .. }, _) => return Err(String::from("a picture spotlight without its GPU panes")),
            (oracle::Spotlight::None, _) => gpu::Spotlight::None,
            (oracle::Spotlight::Timing { frame_us, budget_us }, _) => gpu::Spotlight::Timing { frame_us: *frame_us, budget_us: *budget_us },
            (oracle::Spotlight::Measurement { name, number, unit, bound }, _) => gpu::Spotlight::Measurement { name: *name, number: *number, unit: *unit, bound: *bound },
            (oracle::Spotlight::Input { points: p, hit }, _) => {
                spot_points = points(p);
                gpu::Spotlight::Input { points: &spot_points, hit: *hit }
            }
        };
        input_points = f.run.input.map(|(p, _)| points(p));
        let run = gpu::Run {
            suite: f.run.suite,
            target: f.run.target,
            case_number: f.run.case_number,
            ui_time_ms: f.run.ui_time_ms,
            fps: f.run.fps,
            tiles: &tiles,
            milestones: &milestones,
            selected: f.run.selected,
            spotlight,
            input: f.run.input.zip(input_points.as_deref()).map(|((_, hit), p)| (p, hit)),
            finale: f.run.finale.map(|v| gpu::Finale { slowest: v.slowest, slowest_ms: v.slowest_ms }),
            notice: f.notice,
            ribbon: f.ribbon,
        };
        let painted = self.stage.draw(canvas, &run)
            .map_err(|e| format!("stage paint: {e:?}"))
            .and_then(|()| canvas.wait().map_err(|e| format!("stage paint render: {e}")));
        if painted.is_err() {
            // A lost context (a device reset, or a quarantine) takes the canvas, the glyph
            // atlas and the wall with it: the next frame makes them again.
            self.canvas = None;
            self.stage = gpu::Stage::new();
        }
        painted
    }
}
