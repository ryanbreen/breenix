//! Deterministic run showcase, BreenGL contract section 23.
//! Inputs are immutable result snapshots. The caller owns the UI clock and display handoff.
use crate::{color::Color, font, framebuf::FrameBuf, shapes, ttf_font};
use core::fmt::Write;
use crate::font_api::{CachedFont, Font};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    Pending,
    Running,
    Pass,
    Fail,
    NotReached,
}
#[derive(Clone, Copy)]
pub struct Image<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
}
#[derive(Clone, Copy)]
pub struct Tile<'a> {
    pub id: &'a str,
    pub stamp: Stamp,
    pub key: u64,
    pub thumbnail: Option<Image<'a>>,
}
#[derive(Clone, Copy)]
pub struct Milestone<'a> {
    pub id: &'a str,
    pub first: usize,
    pub end: usize,
}
#[derive(Clone, Copy)]
pub enum Spotlight<'a> {
    None,
    /// A judged surface without a supplied independent reference.
    Actual(Image<'a>),
    Picture {
        expected: Image<'a>,
        actual: Image<'a>,
        first: Option<(usize, usize)>,
        differing: usize,
        bound: usize,
    },
    Timing {
        frame_us: &'a [u32],
        budget_us: u32,
    },
    Measurement { name: &'a str, number: i64, unit: &'a str, bound: Option<(i64, i64)> },
    Input { points: &'a [InputPoint], hit: &'a str },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputPoint { pub x: i32, pub y: i32, pub pressed: bool }

/// Category headings occupy two slots, followed immediately by their cases. The largest
/// integer scale that fits the entire wall is used, without hiding a pending case.
#[derive(Clone, Copy)]
pub struct Wall { pub columns: usize, pub scale: usize }
impl Wall {
    pub fn for_tiles(tiles: &[Tile<'_>]) -> Result<Self, StageError> {
        if tiles.is_empty() { return Err(StageError::Manifest); }
        for scale in (1..=4).rev() {
            let wall = Self { columns: 800 / (40 * scale), scale };
            let slot = wall.slot(tiles, tiles.len() - 1);
            if wall.origin(slot).1 + 24 * scale <= 700 { return Ok(wall); }
        }
        Err(StageError::Manifest)
    }
    pub fn slot(self, tiles: &[Tile<'_>], index: usize) -> usize {
        let mut slot = 0;
        for (i,t) in tiles[..=index].iter().enumerate() {
            if i==0 || category(t.id)!=category(tiles[i-1].id) {
                if slot % self.columns == self.columns-1 { slot += 1; }
                slot += 2;
            }
            if i==index { return slot; }
            slot += 1;
        }
        slot
    }
    pub fn origin(self, slot: usize) -> (usize,usize) {
        (16 + 40*self.scale*(slot%self.columns),112 + (if self.scale == 1 { 24 } else { 26*self.scale })*(slot/self.columns))
    }
}
fn category(id: &str) -> &str { id.split_once('/').map_or(id, |v| v.0) }
#[derive(Clone, Copy)]
pub struct Finale<'a> {
    pub started_ms: u64,
    pub slowest: &'a str,
    pub slowest_ms: u64,
}
impl Finale<'_> {
    pub fn hold_complete(&self, ui_time_ms: u64) -> bool {
        ui_time_ms.saturating_sub(self.started_ms) >= 5000
    }
}
pub struct Run<'a> {
    pub announcement: Option<&'a str>,
    pub suite: &'a str,
    pub target: &'a str,
    pub case_number: usize,
    pub ui_time_ms: u64,
    pub fps: u32,
    pub tiles: &'a [Tile<'a>],
    pub milestones: &'a [Milestone<'a>],
    pub selected: usize,
    pub spotlight: Spotlight<'a>,
    pub input: Option<(&'a [InputPoint], &'a str)>,
    pub finale: Option<Finale<'a>>,
    pub boundary: Observation,
}
/// The observation record fields that constrain the stage's boundary notice.
#[derive(Clone, Copy)]
pub struct Observation { pub backend: u32, pub flags: u32, pub boundary: u32 }
impl Observation {
    pub fn notice(self) -> Result<&'static str, StageError> {
        boundary_notice(self.backend,self.flags,self.boundary)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum StageError {
    Size,
    /// The pinned DejaVu Sans did not parse.
    Font,
    Manifest,
    Image,
    Timing,
    Boundary,
}

const BG: Color = Color::rgb(12, 19, 31);
const HEADER: Color = Color::rgb(24, 39, 59);
const INSET: Color = Color::rgb(18, 29, 44);
const DIM: Color = Color::rgb(161, 183, 204);
const PASS: Color = Color::rgb(26, 112, 77);
const FAIL: Color = Color::rgb(145, 50, 58);
const GREY: Color = Color::rgb(57, 67, 80);
const OUTLINE: Color = Color::rgb(48, 67, 96);
const BAR: Color = Color::rgb(64, 160, 224);

struct Text {
    bytes: [u8; 192],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self {
            bytes: [0; 192],
            len: 0,
        }
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.len == self.bytes.len() {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}
impl core::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for ch in s.bytes() {
            if self.len == self.bytes.len() {
                break;
            }
            self.bytes[self.len] = ch;
            self.len += 1;
        }
        Ok(())
    }
}
/// The pinned DejaVu Sans the stage draws its text in above display scale 1 (section 23.2),
/// parsed on first use; a holder keeps it, and its glyph cache, across frames.
pub struct StageFont(Option<CachedFont>);
impl StageFont {
    pub const fn new() -> Self {
        Self(None)
    }
    /// Parse the font now, so its memory is in place before the first frame.
    pub fn ready(&mut self) -> Result<(), StageError> {
        self.get().map(drop)
    }
    fn get(&mut self) -> Result<&mut CachedFont, StageError> {
        if self.0.is_none() {
            let font = Font::parse(include_bytes!("../../../fonts/DejaVuSans.ttf")).map_err(|_| StageError::Font)?;
            self.0 = Some(CachedFont::with_default_cache(font));
        }
        self.0.as_mut().ok_or(StageError::Font)
    }
}
impl Default for StageFont {
    fn default() -> Self {
        Self::new()
    }
}
/// The display scale a stage surface is drawn at: its size is the logical 1280x800 layout
/// at a whole scale (section 23.2, R4).
fn scale_of(fb: &FrameBuf) -> Result<usize, StageError> {
    let n = fb.width / 1280;
    if n == 0 || n > 8 || fb.width != 1280 * n || fb.height != 800 * n {
        return Err(StageError::Size);
    }
    Ok(n)
}
fn fill(fb: &mut FrameBuf, n: usize, x: usize, y: usize, w: usize, h: usize, c: Color) {
    if w != 0 && h != 0 {
        shapes::fill_rect(fb, (x * n) as i32, (y * n) as i32, (w * n) as i32, (h * n) as i32, c);
    }
}
/// The stage's surface. Coordinates, extents, gaps and borders are in the logical space and
/// multiplied by the display scale `n`; text at legacy scale s is 7s logical pixels per em,
/// drawn at 7sn pixels per em in DejaVu Sans with section 8.3's layout, or with the 5x7 font at
/// scale 1.
struct Canvas<'a> {
    fb: &'a mut FrameBuf,
    n: usize,
    font: Option<&'a mut CachedFont>,
}
impl Canvas<'_> {
    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        fill(self.fb, self.n, x, y, w, h, c);
    }
    fn put(&mut self, x: usize, y: usize, c: Color) {
        if self.n == 1 {
            self.fb.put_pixel(x, y, c);
        } else {
            self.fill(x, y, 1, 1, c);
        }
    }
    /// A one-pixel logical line, drawn n pixels thick.
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
        let n = self.n as i32;
        for dy in 0..n {
            for dx in 0..n {
                shapes::draw_line(self.fb, x0 * n + dx, y0 * n + dy, x1 * n + dx, y1 * n + dy, c);
            }
        }
    }
    /// Text with its pen at (x, y), clipped at the element's right edge, `width` from x.
    fn text(&mut self, bytes: &[u8], x: usize, y: usize, width: usize, scale: usize, color: Color) {
        let height = self.fb.height / self.n;
        self.text_in(bytes, x, y, width, height - y, scale, color);
    }
    /// `text`, also clipped at the element's bottom edge, `height` below y.
    fn text_in(&mut self, bytes: &[u8], x: usize, y: usize, width: usize, height: usize, scale: usize, color: Color) {
        if width == 0 { return; }
        let n = self.n;
        let Some(font) = self.font.as_deref_mut() else {
            self.fb.push_clip(crate::framebuf::Rect::new(x as i32,y as i32,width as i32,(7*scale) as i32));
            for (i, &ch) in bytes.iter().enumerate() {
                if i * 6 * scale >= width { break; }
                font::draw_char(self.fb, ch, x + i * 6 * scale, y, color, scale);
            }
            self.fb.pop_clip();
            return;
        };
        self.fb.push_clip(crate::framebuf::Rect::new(0, (y * n) as i32, ((x + width) * n) as i32, (height * n) as i32));
        ttf_font::draw_text_bytes(self.fb, font, bytes, (x * n) as i32, (y * n) as i32, (7 * scale * n) as f32, color);
        self.fb.pop_clip();
    }
}
impl Image<'_> {
    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= 16384
            && self.height <= 16384
            && self
                .width
                .checked_mul(self.height)
                .and_then(|n| n.checked_mul(4))
                .is_some_and(|n| n == self.pixels.len())
    }
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let o = (y * self.width + x) * 4;
        [self.pixels[o], self.pixels[o + 1], self.pixels[o + 2], 255]
    }
}
fn color(p: [u8; 4]) -> Color {
    Color::rgb(p[2], p[1], p[0])
}
fn stamp(state: Stamp, ui_time_ms: u64) -> Color {
    match state {
        Stamp::Pass => PASS,
        Stamp::Fail => FAIL,
        Stamp::NotReached => GREY,
        Stamp::Pending => OUTLINE,
        Stamp::Running => {
            let q = 500 - (ui_time_ms % 1000).abs_diff(500);
            Color::rgb(64, 128 + (127 * q / 500) as u8, 224)
        }
    }
}
fn counts(run: &Run<'_>) -> Text {
    let passed = run.tiles.iter().filter(|t| t.stamp == Stamp::Pass).count();
    let failed = run.tiles.iter().filter(|t| t.stamp == Stamp::Fail).count();
    let missing = run
        .tiles
        .iter()
        .filter(|t| t.stamp == Stamp::NotReached)
        .count();
    let mut out = Text::new();
    let _ = write!(
        out,
        "{passed} passed  {failed} failed  {missing} not reached"
    );
    out
}
fn validate(fb: &FrameBuf, run: &Run<'_>) -> Result<(), StageError> {
    scale_of(fb)?;
    if run.tiles.is_empty()
        || run.selected >= run.tiles.len()
        || run.case_number == 0
        || run.case_number > run.tiles.len()
        || run.milestones.is_empty()
        || run.milestones.len() > 64
    {
        return Err(StageError::Manifest);
    }
    let mut end = 0;
    for m in run.milestones {
        if m.id.is_empty() || m.first != end || m.end <= m.first || m.end > run.tiles.len() {
            return Err(StageError::Manifest);
        }
        end = m.end;
    }
    if end != run.tiles.len() {
        return Err(StageError::Manifest);
    }
    for tile in run.tiles {
        if tile.id.is_empty() {
            return Err(StageError::Manifest);
        }
        if tile.thumbnail.is_some_and(|im| !im.valid()) {
            return Err(StageError::Image);
        }
    }
    match run.spotlight {
        Spotlight::Picture {
            expected,
            actual,
            first,
            differing,
            ..
        } => {
            if !expected.valid()
                || !actual.valid()
                || expected.width != actual.width
                || expected.height != actual.height
                || differing > actual.width * actual.height
                || first.is_some_and(|(x, y)| x >= actual.width || y >= actual.height)
                || (differing == 0) != first.is_none()
            {
                return Err(StageError::Image);
            }
            let mut count = 0;
            let mut first_pixel = None;
            for y in 0..actual.height { for x in 0..actual.width {
                if expected.pixel(x,y) != actual.pixel(x,y) {
                    count += 1;
                    first_pixel.get_or_insert((x,y));
                }
            } }
            if differing != count || first != first_pixel { return Err(StageError::Image); }
        }
        Spotlight::Timing {
            frame_us,
            budget_us,
        } => {
            if frame_us.is_empty() || frame_us.len() > 64 || budget_us == 0 {
                return Err(StageError::Timing);
            }
        }
        Spotlight::Actual(image) => {if !image.valid(){return Err(StageError::Image);}}
        Spotlight::Input { points, hit } => {
            if points.len()>256 || hit.len()>192 || points.iter().any(|p|p.x.abs_diff(0)>1_000_000 || p.y.abs_diff(0)>1_000_000) {return Err(StageError::Image);}
        }
        Spotlight::Measurement { bound: Some((lo,hi)), .. } if lo>hi => return Err(StageError::Timing),
        Spotlight::Measurement { .. } | Spotlight::None => {}
    }
    if run.input.is_some() && !matches!(run.spotlight, Spotlight::Picture { .. } | Spotlight::Actual(_)) { return Err(StageError::Image); }
    if run.input.is_some_and(|(points,hit)|points.len()>256 || hit.len()>192 || points.iter().any(|p|p.x.abs_diff(0)>1_000_000 || p.y.abs_diff(0)>1_000_000)) {return Err(StageError::Image);}
    let wall=Wall::for_tiles(run.tiles)?;
    if wall.origin(wall.slot(run.tiles,run.tiles.len()-1)).1+24*wall.scale>700 {return Err(StageError::Manifest);}
    run.boundary.notice()?;
    Ok(())
}
/// Draw between cases only, at the display scale the surface's size gives. Validation
/// happens before any pixel changes.
pub fn draw(fb: &mut FrameBuf, run: &Run<'_>) -> Result<(), StageError> {
    draw_with(fb, run, &mut StageFont::new())
}
/// `draw`, with the stage's font kept in `font` across frames.
pub fn draw_with(fb: &mut FrameBuf, run: &Run<'_>, font: &mut StageFont) -> Result<(), StageError> {
    validate(fb, run)?;
    let n = scale_of(fb)?;
    let font = if n == 1 { None } else { Some(font.get()?) };
    let cv = &mut Canvas { fb, n, font };
    cv.fill(0, 0, 1280, 800, BG);
    cv.fill(0, 0, 1280, 96, HEADER);
    let mut title = Text::new();
    let _ = write!(title, "BREENIX / {}", run.suite);
    cv.text(title.bytes(), 24, 16, 1256, 3, Color::WHITE);
    let mut progress = Text::new();
    let _ = write!(
        progress,
        "{}  case {} of {}  elapsed {} ms  fps {}",
        run.target,
        run.case_number,
        run.tiles.len(),
        run.ui_time_ms,
        run.fps
    );
    cv.text(progress.bytes(), 24, 48, 1256, 1, Color::WHITE);
    cv.text(counts(run).bytes(), 24, 68, 1256, 2, Color::WHITE);
    let wall = Wall::for_tiles(run.tiles)?;
    let scale = wall.scale;
    for (i, tile) in run.tiles.iter().enumerate() {
        let slot=wall.slot(run.tiles,i);
        if i==0 || category(tile.id)!=category(run.tiles[i-1].id) {
            let (x,y)=wall.origin(slot-2);
            let end=run.tiles[i..].iter().position(|t|category(t.id)!=category(tile.id)).map_or(run.tiles.len(),|n|i+n);
            let passed=run.tiles[i..end].iter().filter(|t|t.stamp==Stamp::Pass).count();
            cv.fill(x,y,78*scale,24*scale,HEADER);
            cv.text(category(tile.id).as_bytes(),x+2,y+2,76*scale,scale,Color::WHITE);
            let mut count=Text::new(); let _=write!(count,"{passed}/{} pass",end-i);
            cv.text(count.bytes(),x+2,y+14*scale,76*scale,scale,Color::WHITE);
        }
        let (x,y)=wall.origin(slot);
        let c=stamp(tile.stamp,run.ui_time_ms);
        cv.fill(x,y,38*scale,1,c);cv.fill(x,y+24*scale-1,38*scale,1,c);
        cv.fill(x,y,1,24*scale,c);cv.fill(x+38*scale-1,y,1,24*scale,c);
        cv.fill(x+2*scale,y+2*scale,16*scale,12*scale,INSET);
        if let Some(image)=tile.thumbnail { thumbnail(cv,image,x+2*scale,y+2*scale,scale); }
        let mut ordinal=Text::new();let _=write!(ordinal,"{}",i+1);
        cv.text(ordinal.bytes(),x+20*scale,y+2*scale,18*scale,scale,Color::WHITE);
        if tile.stamp!=Stamp::Pending {
            cv.fill(x+1,y+15*scale,38*scale-2,9*scale-1,c);
            let mut key=Text::new();if tile.key>99999{let _=write!(key,">");}else{let _=write!(key,"{}",tile.key);}
            cv.text(key.bytes(),x+2*scale,y+16*scale,34*scale,scale,Color::WHITE);
        }
    }
    draw_spotlight(cv, run);
    if let Some(text) = run.announcement {
        wrapped(cv, text.as_bytes(), 24, 720, 1232, 2);
    }
    if let Some(finale) = run.finale {
        cv.fill(48, 240, 720, 240, HEADER);
        cv.text(b"RUN COMPLETE", 72, 264, 696, 3, Color::WHITE);
        cv.text(counts(run).bytes(), 72, 312, 696, 2, Color::WHITE);
        let mut slowest = Text::new();
        let _ = write!(
            slowest,
            "slowest {} {} ms",
            finale.slowest, finale.slowest_ms
        );
        cv.text(slowest.bytes(), 72, 352, 696, 1, Color::WHITE);
    }
    Ok(())
}
fn thumbnail(cv: &mut Canvas<'_>, image: Image<'_>, left: usize, top: usize, scale: usize) {
    let (w, h) = (image.width.min(16), image.height.min(12));
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x * image.width / w, (x + 1) * image.width / w);
            let (y0, y1) = (y * image.height / h, (y + 1) * image.height / h);
            let n = ((x1 - x0) * (y1 - y0)) as u64;
            let mut sum = [0u64; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = image.pixel(sx, sy);
                    for i in 0..3 {
                        sum[i] += p[i] as u64;
                    }
                }
            }
            let p = [
                ((sum[0] + n / 2) / n) as u8,
                ((sum[1] + n / 2) / n) as u8,
                ((sum[2] + n / 2) / n) as u8,
                255,
            ];
            cv.fill(left+x*scale,top+y*scale,scale,scale,color(p));
        }
    }
}
/// Words kept whole on up to five lines of `width / 6s` character cells, the fifth ending in
/// an ellipsis when the text does not fit.
fn wrapped(cv:&mut Canvas<'_>,bytes:&[u8],x:usize,y:usize,width:usize,scale:usize) {
    let count=width/(6*scale);
    if count < 3 { return; }
    let mut rest = bytes;
    for line in 0..5 {
        while rest.first().is_some_and(u8::is_ascii_whitespace) { rest = &rest[1..]; }
        if rest.is_empty() { break; }
        if line == 4 && rest.len() > count {
            let mut end = count - 3;
            if let Some(space) = rest[..end].iter().rposition(u8::is_ascii_whitespace) { end = space; }
            let mut last = Text::new();
            last.push(&rest[..end]);
            last.push(b"...");
            cv.text(last.bytes(),x,y+line*9*scale,width,scale,Color::WHITE);
            break;
        }
        let end = if rest.len() <= count { rest.len() } else {
            rest[..=count].iter().rposition(u8::is_ascii_whitespace).filter(|n| *n > 0).unwrap_or(count)
        };
        cv.text(&rest[..end],x,y+line*9*scale,width,scale,Color::WHITE);
        rest = &rest[end..];
    }
}
fn case_panel(cv: &mut Canvas<'_>, image: Image<'_>) {
    let n=cv.n;
    for y in 0..250*n {for x in 0..400*n {
        let p=image.pixel(x*image.width/(400*n),y*image.height/(250*n));
        cv.fb.put_pixel(848*n+x,400*n+y,color(p));
    }}
}

fn draw_spotlight(cv: &mut Canvas<'_>, run: &Run<'_>) {
    cv.fill(832, 112, 432, 588, INSET);
    let selected = &run.tiles[run.selected];
    let (group, id) = selected.id.split_once('/').unwrap_or(("", selected.id));
    let mut heading = Text::new();
    let _ = write!(heading, "CASE {} / {}", run.selected + 1, group);
    cv.text(heading.bytes(), 840, 120, 424, 1, Color::WHITE);
    cv.text(id.as_bytes(), 840, 132, 424, 1, Color::WHITE);
    let mut key = Text::new();
    let _ = write!(key, "key {}", selected.key);
    cv.text(key.bytes(), 840, 144, 424, 1, Color::WHITE);
    cv.text(match selected.stamp {Stamp::Pass=>b"PASS",Stamp::Fail=>b"FAIL",Stamp::NotReached=>b"NOT REACHED",_=>b"RUNNING"},848,672,400,3,stamp(selected.stamp,run.ui_time_ms));
    match run.spotlight {
        Spotlight::None => {
            cv.text(if selected.stamp == Stamp::NotReached {b"Case did not run"} else {b"Awaiting case result"},848,360,400,2,DIM);
        }
        Spotlight::Measurement { name, number, unit, bound } => {
            wrapped(cv,name.as_bytes(),848,208,400,2);
            let mut value=Text::new(); let _=write!(value,"{number} {unit}");
            wrapped(cv,value.bytes(),848,280,400,3);
            let mut label=Text::new();
            match bound {
                Some((lo,hi)) if lo==hi => {let _=write!(label,"Expected exactly {lo} {unit}");}
                Some((lo,hi)) => {let _=write!(label,"Bound: {lo} to {hi} {unit}");}
                None => {let _=write!(label,"No numeric bound supplied");}
            }
            wrapped(cv,label.bytes(),848,392,400,2);
        }
        Spotlight::Input { points, hit } => {
            cv.text(if points.is_empty(){b"INPUT / events"}else{b"INPUT / pointer trail"},848,184,400,2,Color::WHITE);
            wrapped(cv,hit.as_bytes(),848,216,400,2);
            let min_x=points.iter().map(|p|p.x).min().unwrap_or(0) as i64;
            let min_y=points.iter().map(|p|p.y).min().unwrap_or(0) as i64;
            let dx=(points.iter().map(|p|p.x).max().unwrap_or(1) as i64-min_x).max(1);
            let dy=(points.iter().map(|p|p.y).max().unwrap_or(1) as i64-min_y).max(1);
            let mapped=|p:&InputPoint|(864+((p.x as i64-min_x)*352/dx) as usize,336+((p.y as i64-min_y)*192/dy) as usize);
            for pair in points.windows(2) {
                let (x,y)=mapped(&pair[0]);let (xx,yy)=mapped(&pair[1]);
                cv.line(x as i32,y as i32,xx as i32,yy as i32,BAR);
            }
            for p in points {let (x,y)=mapped(p);cv.fill(x-2,y-2,5,5,if p.pressed{PASS}else{Color::WHITE});}
            if points.is_empty(){cv.text(b"Events (no pointer)",848,360,400,2,Color::WHITE);}
            if !points.is_empty(){cv.text(b"green: pointer down / white: up",848,568,400,1,DIM);}
        }
        Spotlight::Actual(image) => {
            cv.text(b"actual (reference not supplied)",840,152,424,1,Color::WHITE);
            case_panel(cv,image);
        }
        Spotlight::Picture {
            expected,
            actual,
            first,
            differing,
            bound,
        } => {
            for (i, label) in [b"expected".as_slice(), b"actual", b"difference"]
                .iter()
                .enumerate()
            {
                cv.text(label, 840 + i * 144, 152, 128, 1, Color::WHITE);
            }
            for y in 0..128 {
                for x in 0..128 {
                    let (sx, sy) = (x * actual.width / 128, y * actual.height / 128);
                    let (a, b) = (expected.pixel(sx, sy), actual.pixel(sx, sy));
                    let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap();
                    cv.put(840 + x, 168 + y, color(a));
                    cv.put(984 + x, 168 + y, color(b));
                    cv.put(
                        1128 + x,
                        168 + y,
                        if d == 0 {
                            Color::BLACK
                        } else {
                            Color::rgb(255, 255 - d, 0)
                        },
                    );
                }
            }
            case_panel(cv,actual);
            if let Some((x, y)) = first {
                let (cx, cy) = (x * 128 / actual.width, y * 128 / actual.height);
                for xx in cx.saturating_sub(2)..=(cx + 2).min(127) {
                    cv.put(1128 + xx, 168 + cy, Color::WHITE);
                }
                for yy in cy.saturating_sub(2)..=(cy + 2).min(127) {
                    cv.put(1128 + cx, 168 + yy, Color::WHITE);
                }
            }
            let mut label = Text::new();
            let _ = write!(label, "{differing} pixels differ; bound {bound}");
            cv.text(label.bytes(), 840, 312, 424, 1, Color::WHITE);
        }
        Spotlight::Timing {
            frame_us,
            budget_us,
        } => {
            let max = 2 * budget_us as u64;
            for (i, &us) in frame_us.iter().enumerate() {
                let (left, right) = (
                    848 + 400 * i / frame_us.len(),
                    848 + 400 * (i + 1) / frame_us.len(),
                );
                let height = (192 * (us as u64).min(max) / max) as usize;
                cv.fill(left,
                    528 - height,
                    right - left - 1,
                    height,
                    if us > budget_us { FAIL } else { BAR },
                );
            }
            cv.fill(848, 432, 400, 1, Color::WHITE);
            let mut label = Text::new();
            let _ = write!(label, "frame times us; budget {budget_us}");
            cv.text(label.bytes(), 840, 312, 424, 1, Color::WHITE);
        }
    }
    if let Some((points,hit))=run.input {
        cv.text(if points.is_empty(){b"INPUT / events and handler result"}else{b"INPUT / pointer trail and handler result"},848,336,400,1,Color::WHITE);
        wrapped(cv,hit.as_bytes(),848,352,400,1);
        let min_x=points.iter().map(|p|p.x).min().unwrap_or(0) as i64;
        let min_y=points.iter().map(|p|p.y).min().unwrap_or(0) as i64;
        let dx=(points.iter().map(|p|p.x).max().unwrap_or(1) as i64-min_x).max(1);
        let dy=(points.iter().map(|p|p.y).max().unwrap_or(1) as i64-min_y).max(1);
        let mapped=|p:&InputPoint|(864+((p.x as i64-min_x)*352/dx) as usize,376+((p.y as i64-min_y)*16/dy) as usize);
        for pair in points.windows(2) {
            let (x,y)=mapped(&pair[0]);let (xx,yy)=mapped(&pair[1]);
            cv.line(x as i32,y as i32,xx as i32,yy as i32,BAR);
        }
        for p in points {let (x,y)=mapped(p);cv.fill(x-2,y-2,5,5,if p.pressed{PASS}else{Color::WHITE});}
        if points.is_empty(){cv.text(b"Events (no pointer)",848,376,400,1,Color::WHITE);}
    }
}
/// Decode precisely the observation strength the record supports.
pub fn boundary_notice(
    backend: u32,
    flags: u32,
    boundary: u32,
) -> Result<&'static str, StageError> {
    match (backend, flags, boundary) {
        (1, 1, 1) => Ok("host copy and window outside guest view"),
        (2, 0, 2) => Ok("host scanning outside guest view"),
        (3 | 4, 0, 3) => Ok("host window outside guest view"),
        // Record version 2 of a GPU backend's scanout (contract section 23.8): flags carry
        // only the scanout bit and, for a composed scanout, the composite bit.
        (4 | 5, 1 | 3, 4) => Ok("host renderer and window outside guest view"),
        _ => Err(StageError::Boundary),
    }
}
/// One segment per producer-reached stage; callers must supply the manifest order.
pub fn draw_boot_ribbon(fb: &mut FrameBuf, reached: &[bool]) -> Result<(), StageError> {
    let n = scale_of(fb)?;
    if reached.is_empty() || reached.len() > 416 {
        return Err(StageError::Manifest);
    }
    fill(fb, n, 16, 752, 1248, 16, BG);
    for (i, &lit) in reached.iter().enumerate() {
        let (left, right) = (
            16 + 1248 * i / reached.len(),
            16 + 1248 * (i + 1) / reached.len(),
        );
        fill(fb, n, left,
            752,
            right - left - 2,
            16,
            if lit { PASS } else { OUTLINE },
        );
    }
    Ok(())
}
