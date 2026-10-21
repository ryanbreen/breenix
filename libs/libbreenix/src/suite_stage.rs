//! The runner's real between-case stage and its live self-checks (contract §23).
use super::suite::{check, CaseResult, FrameTiming, JudgedFrame, Outcome, State, Suite};
use crate::{
    gpu::{self, abi::record},
    graphics, process, time,
    window::{self as win, abi},
};
use abi::{op, Plain};
use libgfx_ref::{
    framebuf::{Format, FrameBuf},
    stage::{self, Finale, Image, Milestone, Observation, Spotlight, StageFont, Stamp, Tile},
};
use std::{
    string::String,
    sync::{Arc, Mutex},
    vec::Vec,
};

use crate::ui_clock as clock;
static SHOW: Mutex<Option<Show>> = Mutex::new(None);
/// The stage's DejaVu Sans and its glyph cache, kept across frames.
static FONT: Mutex<StageFont> = Mutex::new(StageFont::new());
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
    input: Option<super::suite::InputTrace>,
    reading: Option<super::suite::Value>,
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

    windows: Vec<(u64, u64)>,
    measured: Option<u64>,
    pace_ms: u64,
}
fn fault_bits() -> u32 {
    std::fs::read_to_string("/proc/breenix/stage-control")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
/// What the stage reads from one scanout observation: core record version 2 (contract
/// section 20.1).
#[derive(Clone, Copy)]
struct Seen {
    backend: u32,
    flags: u32,
    boundary: u32,
    width: usize,
    height: usize,
    present_fence: u64,
}
fn word(rec: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([rec[at], rec[at + 1], rec[at + 2], rec[at + 3]])
}
fn quad(rec: &[u8], at: usize) -> u64 {
    u64::from(word(rec, at)) | u64::from(word(rec, at + 4)) << 32
}
/// The display's current mode, which sizes the stage's screen buffers (ruling B8).
fn mode() -> Result<gpu::abi::Mode, String> {
    let (r, modes) = gpu::modes();
    if r != 0 {
        return Err(std::format!("stage MODES: errno {}", -r));
    }
    modes.modes.get(modes.current as usize).copied().ok_or_else(|| String::from("stage MODES: no current mode"))
}
/// Observe the whole scanout into `pixels` by the 48-byte SCANOUT_READBACK.
fn observe(pixels: &mut Vec<u8>) -> Result<Seen, String> {
    let m = mode()?;
    pixels.resize(m.width as usize * m.height as usize * 4, 0);
    let mut rec = [0u8; record::SIZE];
    let mut rb = abi::Readback::ZERO;
    rb.dst = pixels.as_mut_ptr() as u64;
    rb.dst_len = pixels.len() as u64;
    rb.record = rec.as_mut_ptr() as u64;
    rb.record_len = rec.len() as u32;
    if let Err(e) = win::call(op::SCANOUT_READBACK, &mut rb) {
        return Err(std::format!("stage readback: {e}, reason {}", rb.reason));
    }
    let (width, height) = (word(&rec, record::WIDTH) as usize, word(&rec, record::HEIGHT) as usize);
    if word(&rec, record::VERSION_OFF) != record::VERSION
        || word(&rec, record::SIZE_OFF) as usize != record::SIZE
        || quad(&rec, record::TARGET) != 0
        || word(&rec, record::FLAGS) & 1 == 0
        || word(&rec, record::UNWRITTEN) != 0
        || (width, height) != (m.width as usize, m.height as usize)
    {
        return Err(String::from("stage readback: not a version 2 record of the whole scanout"));
    }
    Ok(Seen {
        backend: word(&rec, record::BACKEND),
        flags: word(&rec, record::FLAGS),
        boundary: word(&rec, record::BOUNDARY),
        width,
        height,
        present_fence: quad(&rec, record::PRESENT_FENCE),
    })
}
fn snapshot() -> Result<(Vec<u8>, Seen), String> {
    let mut pixels = Vec::new();
    let seen = observe(&mut pixels)?;
    Ok((pixels, seen))
}
/// The stage check: the scanout is bound by the stage's own PRESENT (its context fence),
/// and each physical pixel is the stage frame's pixel, rendered at the display's physical
/// size, compared on B, G and R (X normalized, ruling B11).
fn shown(fb: &FrameBuf, present: Option<u64>, pixels: &[u8], seen: &Seen) -> Result<(), String> {
    let fence = present.ok_or("the stage frame was not presented by a GPU PRESENT")?;
    if seen.present_fence != fence {
        return Err(std::format!(
            "the scanout shows present fence {:#x}, not the stage's {fence:#x}",
            seen.present_fence
        ));
    }
    if (seen.width, seen.height) != (fb.width, fb.height) {
        return Err(std::format!(
            "the {}x{} scanout is not the stage's {}x{} frame",
            seen.width, seen.height, fb.width, fb.height
        ));
    }
    for y in 0..fb.height {
        for x in 0..fb.width {
            let p = fb.get_pixel(x, y);
            let o = (y * seen.width + x) * 4;
            if pixels[o..o + 3] != [p.b, p.g, p.r] {
                return Err(std::format!("the scanout differs from the stage frame at physical ({x}, {y})"));
            }
        }
    }
    Ok(())
}
/// The run stage's GPU objects, kept for the run: its context, a mode-sized texture holding
/// the frame rendered at the display's physical size, the mode-sized scanout target it
/// presents, and the buffers each draw reuses. A forked case gets its own, since GPU handles
/// are not inherited.
struct Gpu {
    pid: u64,
    context: u64,
    texture: u64,
    scanout: u64,
    width: u32,
    height: u32,
    frame: Vec<u8>,
    seen: Vec<u8>,
}
static GPU: Mutex<Option<Gpu>> = Mutex::new(None);

impl Gpu {
    /// Create the stage's objects for the current mode, which must be the stage's logical
    /// layout at a whole display scale, and wait, bounded, for the context's warm-up.
    fn new(pid: u64) -> Result<Gpu, String> {
        let m = mode()?;
        if m.scale == 0 || (m.width, m.height) != (1280 * m.scale, 800 * m.scale) {
            return Err(std::format!(
                "the {}x{} mode is not the stage's 1280x800 layout at scale {}",
                m.width, m.height, m.scale
            ));
        }
        let context = gpu::context_create().map_err(|e| std::format!("stage context: errno {}", -e))?;
        let mut g = Gpu {
            pid,
            context,
            texture: 0,
            scanout: 0,
            width: m.width,
            height: m.height,
            frame: Vec::new(),
            seen: Vec::new(),
        };
        let format = gpu::abi::format::B8G8R8A8;
        match gpu::texture_create(format, m.width, m.height) {
            Ok(t) => g.texture = t,
            Err(e) => {
                g.destroy();
                return Err(std::format!("stage texture: errno {}", -e));
            }
        }
        match gpu::scanout_target_create(format, m.width, m.height) {
            Ok(t) => g.scanout = t,
            Err(e) => {
                g.destroy();
                return Err(std::format!("stage scanout target: errno {}", -e));
            }
        }
        // Written now, so the memory record's first sample already counts their pages.
        g.frame.resize(m.width as usize * m.height as usize * 4, 0);
        g.seen.resize(m.width as usize * m.height as usize * 4, 0);
        // The device warms one context at a time, lowest handle first: a stage context
        // still warming would hold the next case's contexts behind it inside its limit.
        let start = time::now_monotonic().map_or(0, |t| t.as_nanos());
        while gpu::counters(g.context).1.state == gpu::abi::context_state::WARMING
            && time::now_monotonic().map_or(0, |t| t.as_nanos()).saturating_sub(start) < 10_000_000_000
        {
            let _ = time::sleep_ms(5);
        }
        Ok(g)
    }

    fn destroy(&self) {
        for handle in [self.scanout, self.texture] {
            if handle != 0 {
                let _ = gpu::resource_destroy(handle);
            }
        }
        let _ = gpu::context_destroy(self.context);
    }

    /// Upload the frame, rendered at the display's physical size, copy it into the scanout
    /// on the GPU, present it, and observe the scanout. Returns the observation and the
    /// present's context fence.
    fn show(&mut self, fb: &FrameBuf) -> Result<(Seen, u64), String> {
        use gpu::abi::{draw, pipeline, purpose, stream::encode};
        if (fb.width, fb.height) != (self.width as usize, self.height as usize) {
            return Err(std::format!("the stage frame is {}x{}, not the {}x{} mode", fb.width, fb.height, self.width, self.height));
        }
        self.frame.clear();
        for y in 0..fb.height {
            for x in 0..fb.width {
                let p = fb.get_pixel(x, y);
                self.frame.extend_from_slice(&[p.b, p.g, p.r, 255]);
            }
        }
        // The one CPU-drawn upload the structure test admits, until gpu-display draws the
        // stage with BreenGL (ruling B24, #2221).
        let (r, _) = gpu::upload(self.context, self.texture, [0, 0, self.width, self.height], &self.frame, self.width * 4, 0, purpose::IMAGE);
        if r != 0 {
            return Err(std::format!("stage upload: errno {}", -r));
        }
        let mut params = [0u32; 16];
        params[2] = 1;
        let mut stream = Vec::new();
        encode::begin(&mut stream, self.scanout);
        draw::encode(&mut stream, pipeline::COPY, 0, self.texture, 0, 0,
            &[draw::Instance { rect: [0, 0, self.width as i32, self.height as i32], params }]);
        let (r, submitted) = gpu::submit(self.context, &stream);
        if r != 0 {
            return Err(std::format!("stage submit: errno {}", -r));
        }
        let (r, waited) = gpu::wait(self.context, submitted.fence, 2_000_000_000);
        if r != 0 {
            return Err(std::format!("stage render: errno {}, status {}", -r, waited.status));
        }
        // The stage holds the display between cases (idempotent while it does) and gives
        // it back while a case runs (`release`).
        graphics::take_over_display().map_err(|e| std::format!("stage display ownership: {e:?}"))?;
        let (r, presented) = gpu::present(self.context, self.scanout);
        // The device's native success answer (gpu.md section 1.8, ruling 47).
        let success = if gpu::caps().1.backend == gpu::abi::backend::SVGA3D_DX { 1 } else { 0x1100 };
        if r != 0 || presented.answer != success {
            return Err(std::format!("stage present: errno {}, answer {:#x}", -r, presented.answer));
        }
        let seen = observe(&mut self.seen)?;
        Ok((seen, presented.fence))
    }
}

/// Create the stage's GPU objects before the memory record's first sample, as the first
/// draw would, so both samples count them, and read the scanout's record into the kept
/// buffer, so the first frame's notice decodes a real record.
pub(super) fn prepare() -> Result<(), String> {
    let pid = process::getpid().map_err(|e| e.to_string())?.raw() as u64;
    let seen = {
        let mut guard = GPU.lock().unwrap();
        if guard.as_ref().is_none_or(|g| g.pid != pid) {
            *guard = Some(Gpu::new(pid)?);
        }
        let g = guard.as_mut().ok_or("stage GPU objects missing")?;
        observe(&mut g.seen)?
    };
    // The font, too, is in both samples.
    FONT.lock().unwrap().ready().map_err(|e| std::format!("stage font: {e:?}"))?;
    if let Some(s) = SHOW.lock().unwrap().as_mut() {
        s.observation = Observation { backend: seen.backend, flags: seen.flags, boundary: seen.boundary };
    }
    Ok(())
}

/// Present the stage frame on the GPU core and check that the scanout shows it.
fn present(fb: &FrameBuf) -> Result<Seen, String> {
    let pid = process::getpid().map_err(|e| e.to_string())?.raw() as u64;
    // As the GPU suite's view does: no test-control point left armed by a case can alter
    // the stage or be spent by it.
    let _ = gpu::tc_disarm_all();
    let mut guard = GPU.lock().unwrap();
    if let Some(g) = guard.as_ref().filter(|g| g.pid == pid) {
        // A quarantined or reset context, or a mode change, needs new objects.
        let (r, c) = gpu::counters(g.context);
        let live = r == 0 && matches!(c.state, gpu::abi::context_state::LIVE | gpu::abi::context_state::WARMING);
        if !live || mode().is_ok_and(|m| (m.width, m.height) != (g.width, g.height)) {
            g.destroy();
            *guard = None;
        }
    }
    // A forked case abandons the runner's handles, which it does not hold, undestroyed.
    if guard.as_ref().is_none_or(|g| g.pid != pid) {
        *guard = Some(Gpu::new(pid)?);
    }
    let g = guard.as_mut().ok_or("stage GPU objects missing")?;
    let shown_frame = g.show(fb).and_then(|(seen, fence)| shown(fb, Some(fence), &g.seen, &seen).map(|()| seen));
    if shown_frame.is_err() {
        // The next draw starts from new objects.
        g.destroy();
        *guard = None;
    }
    shown_frame
}
impl Show {
    fn render(&self, fb: &mut FrameBuf, bits: u32) -> Result<(), String> {
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
        let picture = if self.finale.is_some() && self.first_failure.is_some() {
            self.first_failure.as_ref().and_then(|v| v.1.as_deref())
        } else {
            self.last.as_deref()
        };
        let mut substituted=picture.map(|p|p.pixels.clone()).unwrap_or_default();
        if bits & (1<<13)!=0 && !substituted.is_empty() { substituted[0]^=255; }
        let reading = self.entries[chosen].reading.as_ref();
        let fallback = reading.map_or(Spotlight::None, |v| Spotlight::Measurement {
            name: &v.name, number: if bits & (1<<13) != 0 { v.number ^ 1 } else { v.number }, unit: &v.unit, bound: v.expect,
        });
        let spotlight = picture.map_or(fallback, |p| {
            // The checked surface is available even when a case has no independent reference.
            // Such a case shows only its actual pane; it never invents a matching expectation.
            let actual = Image {
                pixels: if bits & (1<<13)!=0 {&substituted} else {&p.pixels},
                width: p.width,
                height: p.height,
            };
            if let Some(expected) = &p.expected {
                let mut differing = 0;
                let mut first = None;
                for (i, (a, b)) in expected
                    .chunks_exact(4)
                    .zip(actual.pixels.chunks_exact(4))
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
                    bound: p.bound,
                }
            } else {
                Spotlight::Actual(actual)
            }
        });
        let spotlight =
            self.entries[chosen]
                .timing
                .as_ref().filter(|_| picture.is_none())
                .map_or(spotlight, |t| Spotlight::Timing {
                    frame_us: &t.frame_us,
                    budget_us: t.budget_us,
                });
        let mut substituted_input = self.entries[chosen].input.clone();
        if bits & (1<<13) != 0 {
            if let Some(input) = &mut substituted_input { input.hit = "substituted handler result".into(); }
        }
        let input=substituted_input.as_ref();
        let has_picture=picture.is_some();
        let spotlight = if has_picture {spotlight} else {input.map_or(spotlight, |t|
            Spotlight::Input { points: &t.points, hit: &t.hit })};
        let slow = self.entries.iter().max_by_key(|e| e.key).ok_or("stage has no cases")?;
        render_at_ui_time(
            fb,
            &mut stage::Run {
                suite: &self.suite,
                // Record version 2 names the GPU device, which ARM64 QEMU and Parallels
                // share, rather than the target (contract section 20.2).
                target: match self.observation.backend {
                    gpu::abi::backend::VIRGL => "VirGL",
                    gpu::abi::backend::SVGA3D_DX => "SVGA3D DX",
                    _ => "unknown display",
                },
                case_number: self.current + 1,
                ui_time_ms: 0,
                fps: self.fps,
                tiles: &tiles,
                milestones: &groups,
                selected: chosen,
                spotlight,
                input: input.filter(|_|has_picture).map(|t|(t.points.as_slice(),t.hit.as_str())),
                finale: self.finale.map(|started_ms| Finale {
                    started_ms,
                    slowest: &slow.id,
                    slowest_ms: slow.key,
                }),
                boundary: self.observation,
            },
            self.started_ms,
        )
        .map_err(|e| std::format!("stage render: {e:?}"))?;
        let records =
            std::fs::read_to_string("/proc/breenix/boot-ribbon").map_err(|e| e.to_string())?;
        let reached: Vec<_> = records.lines().map(|l| l.starts_with("1|")).collect();
        stage::draw_boot_ribbon(fb, &reached).map_err(|e| std::format!("stage boot ribbon: {e:?}"))
    }
    fn draw(&mut self, bits: u32) -> Result<(), String> {
        // Rendered at the display's physical size (contract section 23.2, ruling B24).
        let m = mode()?;
        let mut fb = FrameBuf::new(m.width as usize, m.height as usize, Format::BGRX).map_err(|e| e.to_string())?;
        // Before the stage's first frame, learn the actual backend from the scanout, so the
        // notice on this same frame decodes a real record.
        if self.observation.backend == 0 {
            let (_, rec) = snapshot()?;
            self.observation = Observation { backend: rec.backend, flags: rec.flags, boundary: rec.boundary };
        }
        self.render(&mut fb, bits)?;
        let rec = present(&fb)?;
        self.observation = Observation {
            backend: rec.backend,
            flags: rec.flags,
            boundary: rec.boundary,
        };
        Ok(())
    }
}
/// No composite by the runner completed inside any of these measured windows.
fn quiet(windows: &[(u64, u64)]) -> bool {
    windows.iter().all(|&(before, after)| before == after)
}
pub(super) fn init(suite: &Suite) -> u64 {
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
                input: None,
                reading: None,
            });
        }
        // Each manifest category gets its own completion light, including the three
        // image categories that share an effort milestone.
        groups.push((String::from(c.id), first, entries.len()));
    }
    let pace_ms = if suite.id == "breengel" { pace_ms() } else { 0 };
    *SHOW.lock().unwrap() = Some(Show {
        suite: String::from(suite.id),
        entries,
        groups,
        current: 0,
        selected: 0,
        last: None,
        first_failure: None,
        // Not yet observed: the first draw reads the scanout's record before rendering.
        observation: Observation {
            backend: 0,
            flags: 0,
            boundary: 0,
        },
        started_ms: clock::now_ns() / 1_000_000,
        finale: None,
        fps: 0,
        windows: Vec::new(),
        measured: None,
        pace_ms,
    });
    pace_ms
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
    values: &[super::suite::Value],
    limit_ms: u64,
    input: Option<super::suite::InputTrace>,
) -> Result<(), String> {
    let mut guard = SHOW.lock().unwrap();
    let s = guard.as_mut().ok_or("stage is not initialized")?;
    if let Some(fps) = fps {
        s.fps = fps;
    }
    let e = &mut s.entries[s.current];
    e.line = String::from(line);
    e.timing = timing;
    if input.is_some() { e.input = input; }
    (e.stamp, e.key) = match outcome {
        Outcome::Pass { ms } => (Stamp::Pass, *ms),
        Outcome::Fail { ms, .. } => (Stamp::Fail, *ms),
        Outcome::Skip { .. } => (Stamp::NotReached, 0),
    };
    e.reading = if matches!(outcome, Outcome::Skip { .. }) { None } else { Some(values.iter().rev().find(|v|v.expect.is_some()).or_else(||values.last()).cloned()
        .unwrap_or(super::suite::Value { name: String::from("Case execution time"), number: e.key as i64,
            unit: String::from("ms"), expect: Some((0,limit_ms as i64)) })) };
    if let Some(f) = frame {
        verify_source(line, &f)?;
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
            source_hash: libpixcheck::sha256(&f.pixels),
            thumb_hash: libpixcheck::sha256(&pixels),
            pixels,
        });
        s.last = Some(Arc::new(f));
    } else if s.selected != s.current {
        s.last = None;
    }
    if matches!(outcome, Outcome::Fail { .. }) && s.first_failure.is_none() {
        s.first_failure = Some((s.current, s.last.clone()));
    }
    s.selected = s.current;
    Ok(())
}
/// Count kernel-published records for a process, not calls or runner timestamps.
fn kernel_serial(pid: u64) -> Result<u64, String> {
    let text = std::fs::read_to_string("/proc/breenix/stage-composites").map_err(|e| e.to_string())?;
    let mut last = 0;
    for line in text.lines() {
        let (owner, serial) = line.split_once(' ').ok_or("bad kernel composite record")?;
        if owner.parse::<u64>().map_err(|_| "bad composite owner")? == pid {
            last = serial.parse::<u64>().map_err(|_| "bad composite serial")?;
        }
    }
    Ok(last)
}
fn verify_source(line: &str, f: &JudgedFrame) -> Result<(), String> {
    let hash: String = libpixcheck::sha256(&f.pixels).iter().map(|b| std::format!("{b:02x}")).collect();
    if !line.split_whitespace().any(|w| w == std::format!("sha256={hash}")) {
        return Err("selected source pixels differ from the emitted CASE SHA-256".into());
    }
    Ok(())
}
pub(super) fn begin() -> Result<(), String> {
    let serial = kernel_serial(process::getpid().map_err(|e| e.to_string())?.raw() as u64)?;
    if let Some(s) = SHOW.lock().unwrap().as_mut() { s.measured = Some(serial); }
    Ok(())
}
/// Before a case runs (ruling B29): the stage keeps the display, and the case's own
/// takeover nests above it, so the matrix stays on screen until the case presents and is
/// restored when it ends, without the console showing between cases. The cases whose property
/// is the kernel's screen itself, or the depth of the ownership stack, start above the
/// kernel's screen instead: the stage gives the display back for them.
pub(super) fn release_for(name: &str) -> Result<(), String> {
    if matches!(name, "composition/ownership-stack" | "panel/kernel-screens" | "panel/boot-ribbon") {
        release()
    } else {
        Ok(())
    }
}

/// The stage gives the display back. Its next draw takes it again; an EPERM means the stage
/// does not hold it.
fn release() -> Result<(), String> {
    match graphics::give_back_display() {
        Ok(()) | Err(crate::error::Error::Os(crate::errno::Errno::EPERM)) => Ok(()),
        Err(e) => Err(std::format!("stage display give-back: {e}")),
    }
}
pub(super) fn end() -> Result<(), String> {
    let serial = kernel_serial(process::getpid().map_err(|e| e.to_string())?.raw() as u64)?;
    if let Some(s) = SHOW.lock().unwrap().as_mut() {
        if let Some(a) = s.measured.take() { s.windows.push((a, serial)); }
    }
    Ok(())
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
pub(super) fn redraw() -> Result<(), String> {
    SHOW.lock().unwrap().as_mut().ok_or("stage missing")?.draw(0)
}
fn pace_ms() -> u64 {
    // QEMU publishes its boot fw_cfg value through procfs; firmware VMs receive
    // the same setting on the runner's private boot disk.
    ["/proc/breenix/pace", "/etc/breenix/pace"].iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .find_map(|s|s.trim().parse::<u32>().ok())
        .map_or(0,u64::from)
}
pub(super) fn completed() -> Result<(), String> {
    let pace_ms = {
        let mut guard=SHOW.lock().unwrap();
        let s=guard.as_mut().ok_or("stage missing")?;
        check(s.measured.is_none(),"spotlight hold entered a measured window").map_err(|e|std::format!("{e:?}"))?;
        s.draw(0)?;
        s.pace_ms
    };
    // Holds advance elapsed UI time but never case timers or frame-time samples.
    if pace_ms>0 {time::sleep_ms(pace_ms).map_err(|e|e.to_string())?;}
    Ok(())
}
pub(super) fn finale() -> Result<(), String> {
    let start = clock::now_ns() / 1_000_000;
    {
        let mut g = SHOW.lock().unwrap();
        let s = g.as_mut().ok_or("stage missing")?;
        s.finale = Some(start);
        s.draw(0)?;
    }
    let watchdog = time::now_monotonic().map_err(|e|e.to_string())?.as_nanos();
    let finale = Finale { started_ms: start, slowest: "", slowest_ms: 0 };
    while !finale.hold_complete(clock::now_ns() / 1_000_000) {
        if time::now_monotonic().map_err(|e|e.to_string())?.as_nanos() - watchdog > 60_000_000_000 {
            return Err("finale UI clock did not complete its 5000 ms hold within the runner deadline".into());
        }
        time::sleep_ms(20).map_err(|e| e.to_string())?;
    }
    Ok(())
}

mod checks;
pub use checks::showcase_check;

/// Render a fixture using the same clock boundary as the live stage.
pub fn render_at_ui_time(fb: &mut FrameBuf, run: &mut stage::Run<'_>, started_ms: u64) -> Result<(), stage::StageError> {
    run.ui_time_ms = (clock::now_ns() / 1_000_000).saturating_sub(started_ms);
    stage::draw_with(fb, run, &mut FONT.lock().unwrap())
}
