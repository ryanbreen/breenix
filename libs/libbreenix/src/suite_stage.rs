//! The runner's real between-case stage and its live self-checks (contract §23).
use super::suite::{check, CaseResult, FrameTiming, JudgedFrame, Outcome, State, Suite};
use crate::{
    process, time,
    window::{self as win, abi},
};
use abi::{op, Plain};
use libgfx::{
    framebuf::{Format, FrameBuf},
    stage::{self, Finale, Image, Milestone, Observation, Spotlight, Stamp, Tile},
};
use std::{
    string::String,
    sync::{Arc, Mutex},
    vec::Vec,
};

#[path = "../../breengel/src/clock.rs"]
pub mod clock;
static SHOW: Mutex<Option<Show>> = Mutex::new(None);
#[derive(Clone)]
struct Picture {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    source_hash: [u8; 32],
    thumb_hash: [u8; 32],
}
impl Picture {
    fn image(&self) -> Image<'_> {
        Image {
            pixels: &self.pixels,
            width: self.width,
            height: self.height,
        }
    }
}
#[derive(Clone)]
struct Entry {
    id: String,
    stamp: Stamp,
    key: u64,
    line: String,
    picture: Option<Picture>,
    timing: Option<FrameTiming>,
}
#[derive(Clone)]
struct Show {
    suite: String,
    entries: Vec<Entry>,
    groups: Vec<(String, usize, usize)>,
    current: usize,
    selected: usize,
    last: Option<Arc<JudgedFrame>>,
    first_failure: Option<(usize, Option<Arc<JudgedFrame>>)>,
    observation: Observation,
    started_ms: u64,
    finale: Option<u64>,
    fps: u32,
    commits: Vec<(u64, u64, u64)>,
    windows: Vec<(u64, u64)>,
    measured: Option<u64>,
}
fn now() -> u64 {
    time::now_monotonic().map_or(0, |t| t.as_nanos() as u64)
}
fn fault_bits() -> u32 {
    std::fs::read_to_string("/proc/breenix/stage-control")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
fn snapshot() -> Result<(Vec<u8>, abi::ObservationRecord), String> {
    let mut pixels = std::vec![0;1280*800*4];
    let mut rec = abi::ObservationRecord::ZERO;
    let mut rb = abi::Readback::ZERO;
    rb.dst = pixels.as_mut_ptr() as u64;
    rb.dst_len = pixels.len() as u64;
    rb.record = &mut rec as *mut _ as u64;
    rb.record_len = core::mem::size_of_val(&rec) as u32;
    win::call(op::SCANOUT_READBACK, &mut rb).map_err(|e| std::format!("stage readback: {e}"))?;
    Ok((pixels, rec))
}
/// Present through the same v2 window/composite/readback path as every graphics client.
fn present(fb: &FrameBuf) -> Result<(u64, abi::ObservationRecord), String> {
    struct Cleanup(Option<u64>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(id) = self.0 {
                let _ = win::destroy(id);
            }
            let _ = unsafe { win::raw_call(op::RELEASE_COMPOSITOR, core::ptr::null_mut(), 0) };
        }
    }
    let mut claim = abi::Claim::ZERO;
    win::call(op::CLAIM_COMPOSITOR, &mut claim).map_err(|e| std::format!("stage claim: {e}"))?;
    let mut clean = Cleanup(None);
    let w = win::create(1280, 800, 2, 1).map_err(|e| std::format!("stage window: {e}"))?;
    clean.0 = Some(w.id);
    win::register(w.id, b"BREENIX / run stage").map_err(|e| e.to_string())?;
    let a = win::acquire(w.id, 1_000_000_000).map_err(|e| e.to_string())?;
    let dest = unsafe {
        core::slice::from_raw_parts_mut(w.buffer[a.buffer as usize] as *mut u8, 1280 * 800 * 4)
    };
    for y in 0..800 {
        for x in 0..1280 {
            let p = fb.get_pixel(x, y);
            let i = (y * 1280 + x) * 4;
            dest[i..i + 4].copy_from_slice(&[p.b, p.g, p.r, 255]);
        }
    }
    win::commit(w.id, a.buffer, &[abi::Rect::new(0, 0, 1280, 800)], None)
        .map_err(|e| e.to_string())?;
    let mut g = abi::Geometry::ZERO;
    g.id = w.id;
    g.flags = abi::geometry_flags::VISIBLE;
    win::call(op::SET_GEOMETRY, &mut g).map_err(|e| e.to_string())?;
    let mut c = abi::Composite::ZERO;
    win::call(op::COMPOSITE, &mut c).map_err(|e| e.to_string())?;
    let mut record_words = [0u64; 67];
    let mut query = abi::RecordQuery {
        serial: c.serial,
        ptr: record_words.as_mut_ptr() as u64,
        len: core::mem::size_of_val(&record_words) as u32,
        reserved: 0,
    };
    win::call(op::COMPOSITE_RECORD, &mut query).map_err(|e| e.to_string())?;
    let header = unsafe {
        core::ptr::read_unaligned(record_words.as_ptr().cast::<abi::CompositeRecordHeader>())
    };
    if header.serial != c.serial || header.flags != 0 || header.count > 16 {
        return Err("stage composite record invalid".into());
    }
    let entries = unsafe {
        core::slice::from_raw_parts(
            record_words.as_ptr().add(3).cast::<abi::CompositeEntry>(),
            header.count as usize,
        )
    };
    if !entries
        .iter()
        .any(|e| e.id == w.id && e.commit_id == 1 && e.pixel_gen == 1)
    {
        return Err("stage window absent from composite record".into());
    }
    let (_, rec) = snapshot()?;
    if rec.serial != c.serial
        || !rec.windows[..rec.window_count as usize]
            .iter()
            .any(|v| v.id == w.id && v.commit_id == 1)
    {
        return Err(String::from(
            "stage composite is missing its completed window",
        ));
    }
    Ok((c.serial, rec))
}
impl Show {
    fn render(&self, fb: &mut FrameBuf, bits: u32, ui_ms: u64) -> Result<(), String> {
        let mut tiles: Vec<Tile<'_>> = self
            .entries
            .iter()
            .map(|e| Tile {
                id: &e.id,
                stamp: e.stamp,
                key: e.key,
                thumbnail: e.picture.as_ref().map(Picture::image),
            })
            .collect();
        // The faults change the actual renderer input, while stored serial results stay intact.
        if bits & (1 << 12) != 0 {
            if let Some(t) = tiles.iter_mut().find(|t| t.stamp == Stamp::Pass) {
                t.stamp = Stamp::Fail;
            }
        }
        let mut substitute = std::vec![0u8;16*12*4];
        for p in substitute.chunks_exact_mut(4) {
            p.copy_from_slice(&[220, 10, 200, 255]);
        }
        if bits & (1 << 13) != 0 {
            if let Some(t) = tiles.iter_mut().find(|t| t.thumbnail.is_some()) {
                t.thumbnail = Some(Image {
                    pixels: &substitute,
                    width: 16,
                    height: 12,
                });
            }
        }
        let groups: Vec<Milestone<'_>> = self
            .groups
            .iter()
            .map(|(id, first, end)| Milestone {
                id,
                first: *first,
                end: *end,
            })
            .collect();
        let chosen = if self.finale.is_some() {
            self.first_failure.as_ref().map_or(self.selected, |v| v.0)
        } else {
            self.selected
        };
        let picture = if self.finale.is_some() {
            self.first_failure.as_ref().and_then(|v| v.1.as_deref())
        } else {
            self.last.as_deref()
        };
        let spotlight = picture.map_or(Spotlight::None, |p| {
            // The checked surface is available even when a case has no independent reference.
            // Such a case shows only its actual pane; it never invents a matching expectation.
            let actual = Image {
                pixels: &p.pixels,
                width: p.width,
                height: p.height,
            };
            if let Some(expected) = &p.expected {
                let mut differing = 0;
                let mut first = None;
                for (i, (a, b)) in expected
                    .chunks_exact(4)
                    .zip(p.pixels.chunks_exact(4))
                    .enumerate()
                {
                    if a[..3] != b[..3] {
                        differing += 1;
                        first.get_or_insert((i % p.width, i / p.width));
                    }
                }
                Spotlight::Picture {
                    expected: Image {
                        pixels: expected,
                        width: p.width,
                        height: p.height,
                    },
                    actual,
                    first,
                    differing,
                    bound: 0,
                }
            } else {
                Spotlight::Actual(actual)
            }
        });
        let spotlight =
            self.entries[chosen]
                .timing
                .as_ref()
                .map_or(spotlight, |t| Spotlight::Timing {
                    frame_us: &t.frame_us,
                    budget_us: t.budget_us,
                });
        let slow = self.entries.iter().max_by_key(|e| e.key).unwrap();
        stage::draw(
            fb,
            &stage::Run {
                suite: &self.suite,
                target: "ARM64 QEMU",
                case_number: self.current + 1,
                ui_time_ms: ui_ms.saturating_sub(self.started_ms),
                fps: self.fps,
                tiles: &tiles,
                milestones: &groups,
                selected: chosen,
                spotlight,
                finale: self.finale.map(|started_ms| Finale {
                    started_ms,
                    slowest: &slow.id,
                    slowest_ms: slow.key,
                }),
                boundary: self.observation,
            },
        )
        .map_err(|e| std::format!("stage render: {e:?}"))?;
        let records =
            std::fs::read_to_string("/proc/breenix/boot-ribbon").map_err(|e| e.to_string())?;
        let reached: Vec<_> = records.lines().map(|l| l.starts_with("1|")).collect();
        stage::draw_boot_ribbon(fb, &reached).map_err(|e| std::format!("stage boot ribbon: {e:?}"))
    }
    fn draw(&mut self, bits: u32) -> Result<(), String> {
        let mut fb = FrameBuf::new(1280, 800, Format::BGRX).map_err(|e| e.to_string())?;
        self.render(&mut fb, bits, clock::now_ns() / 1_000_000)?;
        let start = now();
        let (serial, rec) = present(&fb)?;
        let end = now();
        self.observation = Observation {
            backend: rec.backend,
            flags: rec.flags,
            boundary: rec.boundary,
        };
        self.commits.push((start, end, serial));
        Ok(())
    }
    fn quiet(&self) -> bool {
        self.windows
            .iter()
            .all(|&(a, b)| self.commits.iter().all(|&(x, y, _)| y < a || x > b))
    }
}
pub(super) fn init(suite: &Suite) {
    let mut entries = Vec::new();
    let mut groups: Vec<(String, usize, usize)> = Vec::new();
    for c in suite.categories {
        let first = entries.len();
        for k in c.cases {
            entries.push(Entry {
                id: std::format!("{}/{}", c.id, k.id),
                stamp: Stamp::Pending,
                key: 0,
                line: String::new(),
                picture: None,
                timing: None,
            });
        }
        let group = if ["bmp", "png-decode", "png-encode"].contains(&c.id) {
            "images"
        } else {
            c.id
        };
        if let Some(last) = groups.last_mut().filter(|v| v.0 == group) {
            last.2 = entries.len();
        } else {
            groups.push((String::from(group), first, entries.len()));
        }
    }
    *SHOW.lock().unwrap() = Some(Show {
        suite: String::from(suite.id),
        entries,
        groups,
        current: 0,
        selected: 0,
        last: None,
        first_failure: None,
        observation: Observation {
            backend: 1,
            flags: 1,
            boundary: 1,
        },
        started_ms: clock::now_ns() / 1_000_000,
        finale: None,
        fps: 0,
        commits: Vec::new(),
        windows: Vec::new(),
        measured: None,
    });
}
pub(super) fn draw(states: &[Vec<State>]) -> Result<(), String> {
    let mut guard = SHOW.lock().unwrap();
    let s = guard.as_mut().ok_or("stage is not initialized")?;
    let mut index = 0;
    for cat in states {
        for state in cat {
            if matches!(state, State::Running) {
                s.current = index;
                s.entries[index].stamp = Stamp::Running;
            }
            index += 1;
        }
    }
    s.draw(fault_bits())
}
pub(super) fn record(
    line: &str,
    outcome: &Outcome,
    frame: Option<JudgedFrame>,
    timing: Option<FrameTiming>,
    fps: Option<u32>,
) -> Result<(), String> {
    let mut guard = SHOW.lock().unwrap();
    let s = guard.as_mut().ok_or("stage is not initialized")?;
    if let Some(fps) = fps {
        s.fps = fps;
    }
    let e = &mut s.entries[s.current];
    e.line = String::from(line);
    e.timing = timing;
    (e.stamp, e.key) = match outcome {
        Outcome::Pass { ms } => (Stamp::Pass, *ms),
        Outcome::Fail { ms, .. } => (Stamp::Fail, *ms),
        Outcome::Skip { .. } => (Stamp::NotReached, 0),
    };
    if let Some(f) = frame {
        let source = libpixcheck::Frame::new(
            &f.pixels,
            f.width,
            f.height,
            f.width * 4,
            libpixcheck::Format::Bgrx,
        )
        .map_err(|e| std::format!("thumbnail source: {e:?}"))?;
        let (width, height) = (f.width.min(16), f.height.min(12));
        let pixels = libpixcheck::thumbnail(source, width, height)
            .map_err(|e| std::format!("thumbnail: {e:?}"))?;
        e.picture = Some(Picture {
            width,
            height,
            source_hash: f.sha256,
            thumb_hash: libpixcheck::sha256(&pixels),
            pixels,
        });
        s.last = Some(Arc::new(f));
    } else {
        s.last = None;
    }
    if matches!(outcome, Outcome::Fail { .. }) && s.first_failure.is_none() {
        s.first_failure = Some((s.current, s.last.clone()));
    }
    s.selected = s.current;
    Ok(())
}
pub(super) fn begin() {
    if let Some(s) = SHOW.lock().unwrap().as_mut() {
        s.measured = Some(now());
    }
}
pub(super) fn end() {
    if let Some(s) = SHOW.lock().unwrap().as_mut() {
        if let Some(a) = s.measured.take() {
            s.windows.push((a, now()));
        }
    }
}
fn tick_show(s: &mut Show, bits: u32) -> Result<(), String> {
    if bits & (1 << 14) != 0 {
        s.draw(bits)?;
    }
    Ok(())
}
pub(super) fn tick() -> Result<(), String> {
    let bits = fault_bits();
    tick_show(SHOW.lock().unwrap().as_mut().ok_or("stage missing")?, bits)
}
pub(super) fn finale() -> Result<(), String> {
    let start = clock::now_ns() / 1_000_000;
    {
        let mut g = SHOW.lock().unwrap();
        let s = g.as_mut().ok_or("stage missing")?;
        s.finale = Some(start);
        s.draw(0)?;
    }
    while clock::now_ns() / 1_000_000 < start.saturating_add(5000) {
        time::sleep_ms(20).map_err(|e| e.to_string())?;
    }
    Ok(())
}

mod checks;
pub use checks::showcase_check;
