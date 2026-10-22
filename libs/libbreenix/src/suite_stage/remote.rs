//! A forked case's stage frames, drawn by the runner (rulings B36 and B38). GPU handles are
//! not inherited across fork, so a case's live checks do not draw the stage themselves: they
//! send the runner the variant of the stage they judge and read back the scanout the runner
//! presented. The runner draws it with its own context, canvas, atlas and panes, so no picture
//! is made again in the case's process.
//!
//! A variant is the runner's stage with the fields a check changes: the finale, whether the
//! first failure is kept, the retained judged frame (kept, cleared, or a frame the check made)
//! and the selected entry's stamp, record, timing, input and reading.
use super::*;
use crate::suite::{InputTrace, Value};
use libgfx_ref::stage::InputPoint;

/// The retained judged frame of a variant.
enum Last {
    Keep,
    Clear,
    /// A frame the check made: at most `DRAWN_PIXELS` pixels, drawn by the runner.
    Set(JudgedFrame),
}

/// The fields of the runner's stage a variant replaced, put back once it is drawn.
struct Saved {
    entry: usize,
    stamp: Stamp,
    line: String,
    timing: Option<FrameTiming>,
    input: Option<InputTrace>,
    reading: Option<Value>,
    selected: usize,
    finale: Option<u64>,
    first_failure: Option<(usize, Option<Arc<JudgedFrame>>)>,
    last: Option<Arc<JudgedFrame>>,
}
impl Saved {
    fn restore(self, s: &mut Show) {
        let e = &mut s.entries[self.entry];
        e.stamp = self.stamp;
        e.line = self.line;
        e.timing = self.timing;
        e.input = self.input;
        e.reading = self.reading;
        s.selected = self.selected;
        s.finale = self.finale;
        s.first_failure = self.first_failure;
        s.last = self.last;
    }
}

pub(super) struct Variant {
    finale: Option<u64>,
    first_failure: bool,
    last: Last,
    selected: usize,
    stamp: Stamp,
    line: String,
    timing: Option<FrameTiming>,
    input: Option<InputTrace>,
    reading: Option<Value>,
}

/// The largest frame a check may make for the runner to draw: its panes are drawn as one
/// solid rectangle per source pixel.
pub(super) const DRAWN_PIXELS: usize = 16;

const DRAW: u8 = 1;
const HOLDINGS: u8 = 2;

impl Variant {
    /// `show` as a variant of `base`, the stage the runner had when this process was forked.
    fn of(show: &Show, base: &Show) -> Result<Variant, String> {
        let same = |a: &Option<Arc<JudgedFrame>>, b: &Option<Arc<JudgedFrame>>| a.as_ref().map(|f| f.sha256) == b.as_ref().map(|f| f.sha256);
        let first_failure = match (&show.first_failure, &base.first_failure) {
            (None, _) => false,
            (Some((i, f)), Some((j, g))) if i == j && same(f, g) => true,
            _ => return Err(String::from("a stage variant cannot change the first failure")),
        };
        let last = match &show.last {
            _ if same(&show.last, &base.last) => Last::Keep,
            None => Last::Clear,
            Some(f) if f.width * f.height <= DRAWN_PIXELS => Last::Set((**f).clone()),
            Some(_) => return Err(std::format!("a stage variant's own frame has more than {DRAWN_PIXELS} pixels")),
        };
        let e = &show.entries[show.selected];
        Ok(Variant {
            finale: show.finale,
            first_failure,
            last,
            selected: show.selected,
            stamp: e.stamp,
            line: e.line.clone(),
            timing: e.timing.clone(),
            input: e.input.clone(),
            reading: e.reading.clone(),
        })
    }

    /// Make the runner's stage `s` this variant, returning what it replaced.
    fn apply_to(self, s: &mut Show) -> Result<Saved, String> {
        let e = s.entries.get_mut(self.selected).ok_or("a stage variant selects no entry")?;
        let saved = Saved {
            entry: self.selected,
            stamp: core::mem::replace(&mut e.stamp, self.stamp),
            line: core::mem::replace(&mut e.line, self.line),
            timing: core::mem::replace(&mut e.timing, self.timing),
            input: core::mem::replace(&mut e.input, self.input),
            reading: core::mem::replace(&mut e.reading, self.reading),
            selected: core::mem::replace(&mut s.selected, self.selected),
            finale: core::mem::replace(&mut s.finale, self.finale),
            first_failure: if self.first_failure { s.first_failure.clone() } else { s.first_failure.take() },
            last: match self.last {
                Last::Keep => s.last.clone(),
                Last::Clear => s.last.take(),
                Last::Set(f) => s.last.replace(Arc::new(f)),
            },
        };
        Ok(saved)
    }

    fn encode(&self, w: &mut Writer) {
        w.opt(self.finale.as_ref(), |w, v| w.u64(*v));
        w.u8(self.first_failure as u8);
        match &self.last {
            Last::Keep => w.u8(0),
            Last::Clear => w.u8(1),
            Last::Set(f) => {
                w.u8(2);
                w.u64(f.width as u64);
                w.u64(f.height as u64);
                w.bytes(&f.pixels);
                w.raw(&f.sha256);
                w.opt(f.expected.as_ref(), |w, e| w.bytes(e));
                w.u64(f.bound as u64);
            }
        }
        w.u64(self.selected as u64);
        w.u8(match self.stamp {
            Stamp::Pending => 0,
            Stamp::Running => 1,
            Stamp::Pass => 2,
            Stamp::Fail => 3,
            Stamp::NotReached => 4,
        });
        w.bytes(self.line.as_bytes());
        w.opt(self.timing.as_ref(), |w, t| {
            w.u64(t.frame_us.len() as u64);
            for us in &t.frame_us {
                w.u64(*us as u64);
            }
            w.u64(t.budget_us as u64);
        });
        w.opt(self.input.as_ref(), |w, t| {
            w.u64(t.points.len() as u64);
            for p in &t.points {
                w.u64(p.x as i64 as u64);
                w.u64(p.y as i64 as u64);
                w.u8(p.pressed as u8);
            }
            w.bytes(t.hit.as_bytes());
        });
        w.opt(self.reading.as_ref(), |w, v| {
            w.bytes(v.name.as_bytes());
            w.u64(v.number as u64);
            w.bytes(v.unit.as_bytes());
            w.opt(v.expect.as_ref(), |w, (lo, hi)| {
                w.u64(*lo as u64);
                w.u64(*hi as u64);
            });
        });
    }

    fn decode(r: &mut Reader<'_>) -> Result<Variant, String> {
        let finale = r.opt(|r| r.u64())?;
        let first_failure = r.u8()? != 0;
        let last = match r.u8()? {
            0 => Last::Keep,
            1 => Last::Clear,
            2 => {
                let (width, height) = (r.u64()? as usize, r.u64()? as usize);
                let pixels = r.bytes()?;
                let sha256 = r.array()?;
                let expected = r.opt(|r| r.bytes())?;
                let bound = r.u64()? as usize;
                if width * height > DRAWN_PIXELS || pixels.len() != width * height * 4 || expected.as_ref().is_some_and(|e| e.len() != pixels.len()) {
                    return Err(String::from("a stage variant's own frame is malformed"));
                }
                Last::Set(JudgedFrame { width, height, pixels, sha256, expected, bound })
            }
            _ => return Err(String::from("a stage variant's frame is malformed")),
        };
        let selected = r.u64()? as usize;
        let stamp = match r.u8()? {
            0 => Stamp::Pending,
            1 => Stamp::Running,
            2 => Stamp::Pass,
            3 => Stamp::Fail,
            4 => Stamp::NotReached,
            _ => return Err(String::from("a stage variant's stamp is malformed")),
        };
        let line = r.string()?;
        let timing = r.opt(|r| {
            let n = r.u64()? as usize;
            let frame_us = (0..n).map(|_| r.u64().map(|v| v as u32)).collect::<Result<Vec<_>, _>>()?;
            Ok(FrameTiming { frame_us, budget_us: r.u64()? as u32 })
        })?;
        let input = r.opt(|r| {
            let n = r.u64()? as usize;
            let points = (0..n)
                .map(|_| Ok(InputPoint { x: r.u64()? as i64 as i32, y: r.u64()? as i64 as i32, pressed: r.u8()? != 0 }))
                .collect::<Result<Vec<_>, String>>()?;
            Ok(InputTrace { points, hit: r.string()? })
        })?;
        let reading = r.opt(|r| {
            Ok(Value { name: r.string()?, number: r.u64()? as i64, unit: r.string()?, expect: r.opt(|r| Ok((r.u64()? as i64, r.u64()? as i64)))? })
        })?;
        Ok(Variant { finale, first_failure, last, selected, stamp, line, timing, input, reading })
    }
}

struct Writer(Vec<u8>);
impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn raw(&mut self, v: &[u8]) {
        self.0.extend_from_slice(v);
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.raw(v);
    }
    fn opt<T>(&mut self, v: Option<&T>, f: impl FnOnce(&mut Writer, &T)) {
        self.u8(v.is_some() as u8);
        if let Some(v) = v {
            f(self, v);
        }
    }
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.0.len() < n {
            return Err(String::from("a stage request is truncated"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn array(&mut self) -> Result<[u8; 32], String> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.u64()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn string(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?).map_err(|_| String::from("a stage request's text is not UTF-8"))
    }
    fn opt<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<Option<T>, String> {
        if self.u8()? == 0 { Ok(None) } else { f(self).map(Some) }
    }
}

/// The pictures the runner holds, as a forked case's oracle and checks need them: each
/// tile's thumbnail and the panes held for each judged frame.
#[derive(Clone)]
pub(super) struct Holdings {
    cells: Vec<Option<[u8; 32]>>,
    panes: Vec<([u8; 32], bool)>,
}
impl Holds for Holdings {
    fn cell(&self, tile: usize) -> Option<[u8; 32]> {
        self.cells.get(tile).copied().flatten()
    }
    fn atlas(&self) -> u64 {
        0
    }
    fn panes(&self, hash: &[u8; 32]) -> Option<PaneSet> {
        self.panes.iter().find(|(h, _)| h == hash).map(|&(_, artifacts)| PaneSet { wide: 0, square: 0, artifacts: artifacts.then_some((0, 0)) })
    }
}

/// In a forked case: one request to the runner and its answer.
fn call(request: Vec<u8>) -> Result<Vec<u8>, String> {
    let answer = crate::suite::stage_call(&request)?;
    match answer.split_first() {
        Some((0, rest)) => Ok(rest.to_vec()),
        Some((_, rest)) => Err(String::from_utf8_lossy(rest).into_owned()),
        None => Err(String::from("the runner's stage answer is empty")),
    }
}

/// The pictures the runner held when it drew this process's last requested frame.
static DRAWN: Mutex<Option<Holdings>> = Mutex::new(None);

/// In a forked case: have the runner draw `show` with fault `bits`, as a variant of the stage
/// it had when this process was forked.
pub(super) fn draw(show: &Show, bits: u32) -> Result<(), String> {
    let variant = {
        let base = SHOW.lock().unwrap();
        Variant::of(show, base.as_ref().ok_or("stage is not initialized")?)?
    };
    let mut w = Writer(std::vec![DRAW]);
    w.raw(&bits.to_le_bytes());
    variant.encode(&mut w);
    let held = decode_holdings(&call(w.0)?)?;
    *DRAWN.lock().unwrap() = Some(held);
    Ok(())
}

/// In a forked case: the pictures the runner held when it drew the last frame this process
/// asked for, which that frame shows.
pub(super) fn drawn() -> Result<Holdings, String> {
    DRAWN.lock().unwrap().clone().ok_or_else(|| String::from("the runner has drawn no stage frame for this case"))
}

/// In a forked case: the pictures the runner holds.
pub(super) fn holdings() -> Result<Holdings, String> {
    decode_holdings(&call(std::vec![HOLDINGS])?)
}

fn decode_holdings(answer: &[u8]) -> Result<Holdings, String> {
    let mut r = Reader(answer);
    let cells = (0..r.u64()?).map(|_| r.opt(|r| r.array())).collect::<Result<Vec<_>, _>>()?;
    let panes = (0..r.u64()?).map(|_| Ok((r.array()?, r.u8()? != 0))).collect::<Result<Vec<_>, String>>()?;
    Ok(Holdings { cells, panes })
}

/// In the runner: answer a forked case's request. The answer's first byte is 0 with the
/// result, or 1 with the reason it failed.
pub(crate) fn serve(request: &[u8]) -> Vec<u8> {
    let answered = match request.split_first() {
        Some((&DRAW, rest)) => serve_draw(rest),
        Some((&HOLDINGS, _)) => serve_holdings(),
        _ => Err(String::from("unknown stage request")),
    };
    match answered {
        Ok(mut v) => {
            v.insert(0, 0);
            v
        }
        Err(e) => {
            let mut v = std::vec![1];
            v.extend_from_slice(e.as_bytes());
            v
        }
    }
}

/// Draw the variant and answer with the pictures held while it was drawn. The runner's stage
/// is made the variant in place and put back afterwards, so nothing of it is copied.
fn serve_draw(rest: &[u8]) -> Result<Vec<u8>, String> {
    let mut r = Reader(rest);
    let bits = u32::from_le_bytes(r.take(4)?.try_into().unwrap());
    let variant = Variant::decode(&mut r)?;
    let mut guard = SHOW.lock().unwrap();
    let s = guard.as_mut().ok_or("stage is not initialized")?;
    let saved = variant.apply_to(s)?;
    let drawn = draw_variant(s, &saved, bits);
    saved.restore(s);
    drawn
}

/// Draw `s`, the runner's stage made a variant that replaced `saved`.
fn draw_variant(s: &mut Show, saved: &Saved, bits: u32) -> Result<Vec<u8>, String> {
    // A frame the check made gets panes drawn for it, and its failure artifacts when the
    // variant's entry failed; they are let go once the frame is shown.
    let made = s.last.clone().filter(|f| saved.last.as_ref().is_none_or(|b| b.sha256 != f.sha256));
    if let Some(f) = &made {
        let failing = s.entries[s.selected].stamp == Stamp::Fail;
        let mut guard = GPU.lock().unwrap();
        gpu_here(&mut guard, s.runner)?.drawn(f, failing)?;
    }
    // The case's own test points stay armed: the runner does not disarm them for it. The
    // presents this makes are the case's request, counted from the kernel's records.
    let before = kernel_serial(s.runner)?;
    let drawn = s.paint_served(bits);
    s.requested += kernel_serial(s.runner)?.saturating_sub(before);
    let held = drawn.and_then(|()| serve_holdings());
    if made.is_some() {
        let wanted: Vec<[u8; 32]> = [saved.last.as_ref().map(|f| f.sha256), saved.first_failure.as_ref().and_then(|v| v.1.as_ref().map(|f| f.sha256))]
            .into_iter()
            .flatten()
            .collect();
        if let Some(g) = GPU.lock().unwrap().as_mut() {
            g.prune(&wanted);
        }
    }
    held
}

fn serve_holdings() -> Result<Vec<u8>, String> {
    let guard = GPU.lock().unwrap();
    let g = guard.as_ref().ok_or("the stage holds no GPU objects")?;
    let mut w = Writer(Vec::new());
    w.u64(g.cells.len() as u64);
    for c in &g.cells {
        w.opt(c.as_ref(), |w, h| w.raw(h));
    }
    w.u64(g.panes.len() as u64);
    for p in &g.panes {
        w.raw(&p.hash);
        w.u8(p.artifacts.is_some() as u8);
    }
    Ok(w.0)
}
