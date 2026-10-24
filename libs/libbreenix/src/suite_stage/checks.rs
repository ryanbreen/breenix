//! Independent live checks of the production runner, contract §23.8.
use super::*;
/// Live showcase checks exercise the production renderer and v2 scanout, with one-shot
/// faults consumed by the same code used during the real runner's between-case draw.
pub fn showcase_check(name: &str) -> CaseResult {
    let mut s = SHOW
        .lock()
        .unwrap()
        .clone()
        .ok_or("no runner stage evidence")?;
    if name == "quiet-window" {
        // One measured window per case already recorded, with the runner's between-case
        // composites visible around them; an empty or blind record would read as quiet.
        let recorded = s.entries.iter().filter(|e| !e.line.is_empty()).count();
        check(
            recorded > 0 && s.windows.len() == recorded,
            "the runner did not measure a window for every earlier case",
        )?;
        check(
            s.windows.first().is_some_and(|w| w.0 > 0)
                && s.windows.windows(2).all(|pair| pair[1].0 > pair[0].1),
            "the runner's between-case composites are missing from the kernel records",
        )?;
        check(
            quiet(&s.windows),
            "a runner composite overlaps an earlier measured case",
        )?;
        // The production window bracket and tick, with and without a planted in-window draw.
        for faulty in [true, false] {
            if faulty {
                crate::test_control::arm(14, process::getpid()?.raw() as u64, 0)?;
            }
            begin().map_err(crate::suite::CaseError::Fail)?;
            tick().map_err(crate::suite::CaseError::Fail)?;
            end().map_err(crate::suite::CaseError::Fail)?;
            let planted = SHOW
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|show| show.windows.last().copied())
                .ok_or("the planted window was not measured")?;
            check(
                quiet(&[planted]) != faulty,
                "runner in-window draw detection disagrees with the composite records",
            )?;
            crate::test_control::disarm_all()?;
        }
        return Ok(());
    }
    if name == "judged-frame-shown" {
        let pixels = std::vec![0,0,0,255];
        let hash = libpixcheck::sha256(&pixels);
        let line = std::format!("sha256={}", hash.iter().map(|b| std::format!("{b:02x}")).collect::<String>());
        let mut frame = JudgedFrame { width:1,height:1,pixels,sha256:hash,expected:None,bound:0 };
        verify_source(&line,&frame).map_err(crate::suite::CaseError::Fail)?;
        frame.pixels[0] ^= 255;
        check(verify_source(&line,&frame).is_err(), "wrong source selected at record() was accepted")?;
        // The latest judged frame is retained whole: its tile must be the thumbnail of
        // exactly those pixels, under the digest its CASE record printed.
        let last = s.last.clone().ok_or("no retained judged frame to recompute")?;
        let entry = &s.entries[s.selected];
        let stored = entry.picture.as_ref().ok_or("the retained judged frame has no tile")?;
        verify_source(&entry.line, &last).map_err(crate::suite::CaseError::Fail)?;
        let source = libpixcheck::Frame::new(&last.pixels, last.width, last.height, last.width * 4, libpixcheck::Format::Bgrx)
            .map_err(|e| std::format!("thumbnail source: {e:?}"))?;
        let thumb = libpixcheck::thumbnail(source, stored.width, stored.height)
            .map_err(|e| std::format!("thumbnail: {e:?}"))?;
        check(thumb == stored.pixels, "a tile thumbnail is not the box filter of its judged frame")?;
    }
    // The production finale must select the same retained frame on an all-pass run.
    if name=="judged-frame-shown" {
        let mut final_show=s.clone();final_show.finale=Some(0);
        // Exercise the all-pass selection path even if an earlier real case
        // failed without supplying a picture; that finale correctly shows words.
        final_show.first_failure=None;
        // Rendered at the display's physical size, as the live stage is (ruling B8).
        let d=Dims::of_mode().map_err(crate::suite::CaseError::Fail)?;
        let mut fb=FrameBuf::new(1280*d.n,800*d.n,Format::BGRX).map_err(|e|e.to_string())?;
        let held=remote::holdings().map_err(crate::suite::CaseError::Fail)?;
        final_show.oracle(&mut fb,&held).map_err(crate::suite::CaseError::Fail)?;
        check_spotlight(&final_show,&fb_bytes(&fb),Dims{n:d.n,width:fb.width},&held).map_err(crate::suite::CaseError::Fail)?;
        let mut f=JudgedFrame{width:1,height:1,pixels:std::vec![10,20,30,255],sha256:[0;32],expected:Some(std::vec![10,25,30,255]),bound:0};
        f.sha256=libpixcheck::sha256(&f.pixels);
        let mut probe=s.clone();probe.first_failure=None;probe.finale=None;
        probe.entries[probe.selected].timing=None;probe.entries[probe.selected].input=None;
        // A frame that differs from its reference is a failing case's (ruling B36: only then
        // are its expected and difference panes shown, as failure artifacts).
        probe.entries[probe.selected].stamp=Stamp::Fail;
        probe.entries[probe.selected].published=true;
        probe.entries[probe.selected].line=std::format!(" sha256={}",f.sha256.iter().map(|b|std::format!("{b:02x}")).collect::<String>());
        let probe_hash=f.sha256;
        probe.last=Some(Arc::new(f));probe.draw(0).map_err(crate::suite::CaseError::Fail)?;
        let (mut px,seen)=snapshot().map_err(crate::suite::CaseError::Fail)?;
        let d=Dims::of(&seen);
        let held=remote::drawn().map_err(crate::suite::CaseError::Fail)?;
        check(held.panes(&probe_hash).is_some_and(|p|p.artifacts.is_some()),"the runner did not draw the check's failing picture with its expected and difference panes")?;
        check_spotlight(&probe,&px,d,&held).map_err(crate::suite::CaseError::Fail)?;
        for (x,y) in [(840,168),(984,168),(1128,168),(1136,176),(840,312),(849,401)] {
            // Each substitution is one changed byte, put back once checked.
            px[d.at(x,y)]^=255;let caught=check_spotlight(&probe,&px,d,&held).is_err();px[d.at(x,y)]^=255;
            check(caught,"spotlight accepted a substituted pane, heat map, cross or bound")?;
        }
    }
    if name == "judged-frame-shown" { exercise_details(&s)?; }
    let point = match name {
        "judged-frame-shown" => Some(13),
        "serial-agreement" => Some(12),
        "boundary-notice" => None,
        _ => return Err("unknown live stage check".into()),
    };
    let ids:Vec<_>=s.entries.iter().map(|e|e.id.clone()).collect();
    let tiles:Vec<_>=ids.iter().map(|id|Tile{id,stamp:Stamp::Pending,key:0,thumbnail:None}).collect();
    let wall=stage::Wall::for_tiles(&tiles).map_err(|e|std::format!("wall: {e:?}"))?;
    for faulty in if point.is_some() {
        &[true, false][..]
    } else {
        &[false][..]
    } {
        if *faulty {
            crate::test_control::arm(point.unwrap(), process::getpid()?.raw() as u64, 0)?;
        }
        s.draw(fault_bits())
            .map_err(crate::suite::CaseError::Fail)?;
        let (pixels, rec) = snapshot().map_err(crate::suite::CaseError::Fail)?;
        let d = Dims::of(&rec);
        // Publication recorded at the case boundary determines which pictures are required.
        // Missing GPU holdings must not turn a published picture into an empty expectation.
        let held = remote::drawn().map_err(crate::suite::CaseError::Fail)?;
        let mut mismatch = false;
        match name {
            "judged-frame-shown" => {
                let mut n = 0;
                for (i, e) in s.entries.iter().enumerate() {
                    if let Some(p) = &e.picture {
                        let shown = e.published;
                        check(!shown || held.cell(i) == Some(p.source_hash),
                            "the runner lost a published tile's GPU source")?;
                        n += usize::from(shown);
                        let hash: String = p
                            .source_hash
                            .iter()
                            .map(|b| std::format!("{b:02x}"))
                            .collect();
                        check(
                            e.line.contains(&std::format!(" sha256={hash}")),
                            "tile snapshot hash differs from its stored CASE bytes",
                        )?;
                        check(
                            libpixcheck::sha256(&p.pixels) == p.thumb_hash,
                            "thumbnail snapshot changed after verification",
                        )?;
                        let (x0,y0)=wall.origin(wall.slot(&tiles,i));
                        let scale=wall.scale;
                        let mut expected=Region::new(d,x0+2*scale,y0+2*scale,p.width*scale,p.height*scale,Color::rgb(18,29,44))?;
                        if shown { thumbnail(&mut expected,p,0,0,scale); }
                        mismatch |= expected.compare(d,&pixels).is_err();
                    }
                }
                check(n > 0, "no published GPU picture to check")?;
                // Independently compute every displayed source pixel, heat-map channel,
                // first-mismatch cross and caption; a correct tile cannot conceal a false spotlight.
                let spotlight_mismatch = check_spotlight(&s,&pixels,d,&held).is_err();
                check(spotlight_mismatch == *faulty, "spotlight fault was concealed by thumbnail detection")?;
                mismatch |= spotlight_mismatch;
            }
            "serial-agreement" => {
                let mut font = Oracle::new();
                for (i, e) in s.entries.iter().enumerate() {
                    if e.line.is_empty() {
                        continue;
                    }
                    let status = match e.stamp {
                        Stamp::Pass => "PASS",
                        Stamp::Fail => "FAIL",
                        Stamp::NotReached => "SKIP",
                        _ => return Err("CASE bytes have a pending tile".into()),
                    };
                    check(
                        e.line.starts_with(&std::format!(
                            "SUITE {} CASE {} {status}",
                            s.suite,
                            e.id
                        )),
                        "tile id or verdict differs from its CASE bytes",
                    )?;
                    if e.stamp != Stamp::NotReached {
                        check(
                            e.line
                                .split_whitespace()
                                .any(|v| v == std::format!("ms={}", e.key)),
                            "tile key differs from its CASE milliseconds",
                        )?;
                    }
                    let (left,top)=wall.origin(wall.slot(&tiles,i));
                    let scale=wall.scale;
                    let words: Vec<_> = e.line.split_whitespace().collect();
                    let colour = match words.get(4).copied() {
                        Some("PASS") => Color::rgb(26, 112, 77),
                        Some("FAIL") => Color::rgb(145, 50, 58),
                        Some("SKIP") => Color::rgb(57, 67, 80),
                        _ => return Err("invalid stored CASE verdict".into()),
                    };
                    // Section 23.3's tile, from its CASE bytes: border, inset, thumbnail,
                    // ordinal, key strip and key.
                    let mut expected = Region::new(d, left, top, 38*scale, 24*scale, Color::rgb(12, 19, 31))?;
                    expected.border(38*scale, 24*scale, colour);
                    expected.fill(2*scale, 2*scale, 16*scale, 12*scale, Color::rgb(18, 29, 44));
                    if let Some(p) = e.picture.as_ref().filter(|_| e.published) {
                        check(held.cell(i) == Some(p.source_hash),
                            "the runner lost a published tile's GPU source")?;
                        thumbnail(&mut expected, p, 2*scale, 2*scale, scale);
                    }
                    expected.text(&mut font, std::format!("{}", i + 1).as_bytes(), 20*scale, 2*scale, 18*scale, scale, Color::WHITE);
                    expected.fill(1, 15*scale, 38*scale-2, 9*scale-1, colour);
                    let number = words
                        .iter()
                        .find_map(|w| w.strip_prefix("ms="))
                        .unwrap_or("0");
                    let n = number
                        .parse::<u64>()
                        .map_err(|_| "invalid CASE milliseconds")?;
                    let key = if n > 99999 { ">" } else { number };
                    expected.text(&mut font, key.as_bytes(), 2*scale, 16*scale, 34*scale, scale, Color::WHITE);
                    mismatch |= expected.compare(d, &pixels).is_err();
                }
            }
            "boundary-notice" => {
                // Section 23.8: the GPU backends' record version 2 rows; the legacy CPU
                // backends and boundary codes are retired (section 20.2).
                match (rec.backend, rec.flags, rec.boundary) {
                    (4 | 5, 1 | 3, 4) => {},
                    _ => return Err("invalid observation boundary combination".into()),
                }
                // The operator removed the caption and both footer bands.
                let expected = Region::new(d, 0, 700, 1280, 100, Color::rgb(12, 19, 31))?;
                mismatch |= expected.compare(d, &pixels).is_err();
            }
            _ => unreachable!(),
        }
        check(
            mismatch == *faulty,
            "stage scanout fault detection or clean agreement failed",
        )?;
        if let Some(p) = point {
            if *faulty {
                check(
                    crate::test_control::fired(p)? > 0,
                    "stage fault never fired",
                )?;
            }
        }
        crate::test_control::disarm_all()?;
    }
    Ok(())
}

use libgfx_ref::color::Color;

/// The scale and physical width of the stage the live checks read (ruling B8): logical
/// pixel (x, y) is the n x n block of device pixels at (x n, y n) of a `width`-wide scanout.
#[derive(Clone, Copy)]
struct Dims {
    n: usize,
    width: usize,
}
impl Dims {
    fn of(seen: &Seen) -> Dims {
        Dims { n: (seen.width / 1280).max(1), width: seen.width }
    }
    fn of_mode() -> Result<Dims, String> {
        let m = mode()?;
        Ok(Dims { n: (m.width as usize / 1280).max(1), width: m.width as usize })
    }
    /// The byte offset of logical pixel (x, y)'s first device pixel.
    fn at(self, x: usize, y: usize) -> usize {
        (y * self.n * self.width + x * self.n) * 4
    }
}

/// The reference text layer the expectations are drawn with: section 8.3's DejaVu Sans at
/// 7 x legacy scale x display scale pixels per em above display scale 1, the 5x7 font at 1.
/// The parsed font and its glyphs are kept for the process's next expectation.
struct Oracle(Option<libgfx_ref::ttf_font::CachedFont>);
std::thread_local! {
    static KEPT_FONT: core::cell::RefCell<Option<libgfx_ref::ttf_font::CachedFont>> = const { core::cell::RefCell::new(None) };
}
impl Drop for Oracle {
    fn drop(&mut self) {
        if let Some(font) = self.0.take() {
            KEPT_FONT.with(|k| *k.borrow_mut() = Some(font));
        }
    }
}
impl Oracle {
    fn new() -> Oracle {
        Oracle(KEPT_FONT.with(|k| k.borrow_mut().take()))
    }
    fn font(&mut self) -> &mut libgfx_ref::ttf_font::CachedFont {
        self.0.get_or_insert_with(|| {
            let font = libgfx_ref::ttf_font::Font::parse(include_bytes!("../../../../fonts/DejaVuSans.ttf")).expect("pinned DejaVu Sans");
            libgfx_ref::ttf_font::CachedFont::new(font, 256)
        })
    }
    /// The logical rows one line of text at `scale` occupies.
    fn rows(&mut self, n: usize, scale: usize) -> usize {
        if n == 1 {
            return 7 * scale;
        }
        let m = self.font().metrics((7 * scale * n) as f32);
        ((m.ascender - m.descender + m.line_gap).ceil() as usize).div_ceil(n)
    }
}

/// An expected region of the stage, `w` x `h` logical pixels at logical (x, y), drawn at the
/// display's physical size from section 23's rectangles and formulas with the reference
/// layer. Coordinates given to its methods are logical and relative to the region.
struct Region {
    fb: FrameBuf,
    n: usize,
    x: usize,
    y: usize,
}
impl Region {
    fn new(d: Dims, x: usize, y: usize, w: usize, h: usize, background: Color) -> Result<Region, String> {
        let fb = FrameBuf::new_filled(w * d.n, h * d.n, Format::BGRX, background).map_err(|e| e.to_string())?;
        Ok(Region { fb, n: d.n, x, y })
    }
    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        let n = self.n as i32;
        libgfx_ref::shapes::fill_rect(&mut self.fb, x as i32 * n, y as i32 * n, w as i32 * n, h as i32 * n, c);
    }
    fn put(&mut self, x: usize, y: usize, c: Color) {
        self.fill(x, y, 1, 1, c);
    }
    /// A one-logical-pixel border of the `w` x `h` box at the region's origin.
    fn border(&mut self, w: usize, h: usize, c: Color) {
        self.fill(0, 0, w, 1, c);
        self.fill(0, h - 1, w, 1, c);
        self.fill(0, 0, 1, h, c);
        self.fill(w - 1, 0, 1, h, c);
    }
    /// Text with its pen at (x, y), clipped at its element's right edge, `width` from x.
    fn text(&mut self, font: &mut Oracle, t: &[u8], x: usize, y: usize, width: usize, scale: usize, c: Color) {
        let height = self.fb.height / self.n;
        self.text_in(font, t, x, y, width, height - y, scale, c);
    }
    /// `text`, also clipped at its element's bottom edge, `height` below y.
    fn text_in(&mut self, font: &mut Oracle, t: &[u8], x: usize, y: usize, width: usize, height: usize, scale: usize, c: Color) {
        let n = self.n;
        if n == 1 {
            self.fb.push_clip(libgfx_ref::framebuf::Rect::new(x as i32, y as i32, width as i32, (7 * scale) as i32));
            libgfx_ref::font::draw_text(&mut self.fb, t, x, y, c, scale);
        } else {
            self.fb.push_clip(libgfx_ref::framebuf::Rect::new(0, (y * n) as i32, ((x + width) * n) as i32, (height * n) as i32));
            libgfx_ref::ttf_font::draw_text_bytes(&mut self.fb, font.font(), t, (x * n) as i32, (y * n) as i32, (7 * scale * n) as f32, c);
        }
        self.fb.pop_clip();
    }
    /// A one-logical-pixel line, n device pixels thick.
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
        let n = self.n as i32;
        for dy in 0..n {
            for dx in 0..n {
                libgfx_ref::shapes::draw_line(&mut self.fb, x0 * n + dx, y0 * n + dy, x1 * n + dx, y1 * n + dy, c);
            }
        }
    }
    /// The region against the scanout bytes, B, G and R, with X 255.
    fn compare(&self, d: Dims, bytes: &[u8]) -> Result<(), String> {
        for yy in 0..self.fb.height {
            for xx in 0..self.fb.width {
                let p = self.fb.get_pixel(xx, yy);
                let o = ((self.y * d.n + yy) * d.width + self.x * d.n + xx) * 4;
                if bytes.get(o..o + 4) != Some(&[p.b, p.g, p.r, 255][..]) {
                    return Err(std::format!("stage detail differs at device pixel {},{}", self.x * d.n + xx, self.y * d.n + yy));
                }
            }
        }
        Ok(())
    }
}

/// A tile thumbnail's pixels, each enlarged to `scale` x `scale` logical pixels at (x, y).
fn thumbnail(r: &mut Region, p: &Picture, x: usize, y: usize, scale: usize) {
    for ty in 0..p.height {
        for tx in 0..p.width {
            let o = (ty * p.width + tx) * 4;
            r.fill(x + tx * scale, y + ty * scale, scale, scale, Color::rgb(p.pixels[o + 2], p.pixels[o + 1], p.pixels[o]));
        }
    }
}

fn fb_bytes(fb:&FrameBuf)->Vec<u8> {
    let mut bytes=Vec::with_capacity(fb.width*fb.height*4);
    for y in 0..fb.height {for x in 0..fb.width {let p=fb.get_pixel(x,y);bytes.extend_from_slice(&[p.b,p.g,p.r,255]);}}
    bytes
}
/// Literal spotlight formulas from section 23, required for a published GPU source.
/// CPU-only cases have no GPU panes (B36, #2274).
fn check_spotlight(s:&Show,bytes:&[u8],d:Dims,held:&dyn Holds)->Result<(),String> {
    let chosen=s.first_failure.as_ref().filter(|_|s.finale.is_some()).map_or(s.selected,|v|v.0);
    let frame=if s.finale.is_some() && s.first_failure.is_some(){s.first_failure.as_ref().and_then(|v|v.1.as_deref())}else{s.last.as_deref()};
    let Some(frame) = frame else { return check_nonpicture(s, chosen, bytes, d); };
    if !s.entries[chosen].published { return check_nonpicture(s, chosen, bytes, d); }
    let panes = held.panes(&frame.sha256).ok_or("the runner lost the published spotlight's GPU source")?;
    verify_source(&s.entries[chosen].line,frame)?;
    let colour=|p:&[u8]|Color::rgb(p[2],p[1],p[0]);
    // Ruling B36: expected and difference panes for a frame that matches its reference, or
    // for a failing case with its failure artifacts; otherwise the actual pane alone.
    let shows_reference=|e:&[u8]|(s.entries[chosen].stamp==Stamp::Fail && panes.artifacts.is_some()) || e.chunks_exact(4).zip(frame.pixels.chunks_exact(4)).all(|(a,b)|a[..3]==b[..3]);
    if let Some(expected)=frame.expected.as_ref().filter(|e|shows_reference(e)) {
        let mut first=None;let mut differing=0;
        for (i,(a,b)) in frame.pixels.chunks_exact(4).zip(expected.chunks_exact(4)).enumerate(){if a!=b{differing+=1;first.get_or_insert((i%frame.width,i/frame.width));}}
        // The expected, actual and difference panes, each nearest-scaled from the whole source.
        let inset=Color::rgb(18,29,44);
        let mut panes=[Region::new(d,840,168,128,128,inset)?,Region::new(d,984,168,128,128,inset)?,Region::new(d,1128,168,128,128,inset)?];
        for y in 0..128 {for x in 0..128 {
            let i=((y*frame.height/128)*frame.width+x*frame.width/128)*4;
            let a=&frame.pixels[i..i+4];let e=&expected[i..i+4];
            let delta=(0..3).map(|c|a[c].abs_diff(e[c])).max().unwrap();
            let cross=first.is_some_and(|(fx,fy)|{let (cx,cy)=(fx*128/frame.width,fy*128/frame.height);(y==cy && x.abs_diff(cx)<=2)||(x==cx && y.abs_diff(cy)<=2)});
            let diff=if cross{Color::WHITE}else if delta==0{Color::BLACK}else{Color::rgb(255,255-delta,0)};
            panes[0].put(x,y,colour(e));panes[1].put(x,y,colour(a));panes[2].put(x,y,diff);
        }}
        for pane in &panes {
            pane.compare(d,bytes).map_err(|_|String::from("spotlight differs from judged actual, reference or heat map"))?;
        }
        let mut font=Oracle::new();
        let rows=font.rows(d.n,1);
        let mut label=Region::new(d,840,312,424,rows,Color::rgb(18,29,44))?;
        label.text(&mut font,std::format!("{differing} pixels differ; bound {}",frame.bound).as_bytes(),0,0,424,1,Color::WHITE);
        label.compare(d,bytes).map_err(|_|String::from("spotlight bound differs"))?;
    }
    // B50's fixed panel compares physical pixels, including odd coordinates at scale 2.
    for y in 0..250*d.n { for x in 0..400*d.n {
        let source = ((y*frame.height/(250*d.n))*frame.width+x*frame.width/(400*d.n))*4;
        let at = ((400*d.n+y)*d.width+848*d.n+x)*4;
        check(bytes[at..at+3] == frame.pixels[source..source+3], "B50: inset target differs from the judged frame at native resolution").map_err(|e|std::format!("{e:?}"))?;
    }}
    if let Some(input) = &s.entries[chosen].input { check_input(bytes, input, true, d)?; }
    Ok(())
}

/// Words kept whole on up to five lines of `width / 6s` character cells, the fifth ending in
/// an ellipsis when the text does not fit.
fn words(r: &mut Region, font: &mut Oracle, text: &str, x: usize, y: usize, width: usize, scale: usize) {
    let columns = width / (6*scale);
    let mut tail = text.trim_start();
    for row in 0..5 {
        if tail.is_empty() { break; }
        let mut line = String::new();
        if row == 4 && tail.len() > columns {
            let prefix = &tail.as_bytes()[..columns-3];
            let end = prefix.iter().rposition(u8::is_ascii_whitespace).unwrap_or(prefix.len());
            line.push_str(core::str::from_utf8(&prefix[..end]).unwrap()); line.push_str("...");
            tail = "";
        } else {
            let mut used = 0;
            for word in tail.split_whitespace() {
                let required = word.len() + usize::from(!line.is_empty());
                if line.len()+required > columns { break; }
                if !line.is_empty() { line.push(' '); }
                line.push_str(word); used = line.len();
            }
            if used == 0 { used = columns.min(tail.len()); line.push_str(&tail[..used]); }
            tail = tail[used..].trim_start();
        }
        r.text(font,line.as_bytes(),x,y+row*9*scale,width,scale,Color::WHITE);
    }
}
fn check_input(bytes: &[u8], input: &super::super::suite::InputTrace, overlay: bool, d: Dims) -> Result<(), String> {
    let (top,height,scale) = if overlay {(336,64,1)} else {(184,416,2)};
    let mut font=Oracle::new();
    let mut expected=Region::new(d,848,top,416,height,Color::rgb(18,29,44))?;
    let heading = match (overlay,input.points.is_empty()) {
        (true,true)=>"INPUT / events and handler result",
        (true,false)=>"INPUT / pointer trail and handler result",
        (false,true)=>"INPUT / events",
        (false,false)=>"INPUT / pointer trail",
    };
    expected.text(&mut font,heading.as_bytes(),0,0,400,scale,Color::WHITE);
    words(&mut expected,&mut font,&input.hit,0,if overlay{16}else{32},400,scale);
    let points=&input.points;
    if points.is_empty() {
        expected.text(&mut font,b"Events (no pointer)",0,if overlay{40}else{176},400,if overlay{1}else{2},Color::WHITE);
    } else {
        let (min_x,max_x)=(points.iter().map(|p|p.x as i64).min().unwrap(),points.iter().map(|p|p.x as i64).max().unwrap());
        let (min_y,max_y)=(points.iter().map(|p|p.y as i64).min().unwrap(),points.iter().map(|p|p.y as i64).max().unwrap());
        let map=|p:&stage::InputPoint|(16+(p.x as i64-min_x)*352/(max_x-min_x).max(1),(if overlay{40}else{152})+(p.y as i64-min_y)*(if overlay{16}else{192})/(max_y-min_y).max(1));
        for pair in points.windows(2) {
            let (x,y)=map(&pair[0]);let (xx,yy)=map(&pair[1]);
            expected.line(x as i32,y as i32,xx as i32,yy as i32,Color::rgb(64,160,224));
        }
        for point in points {
            let (x,y)=map(point);
            expected.fill(x as usize-2,y as usize-2,5,5,if point.pressed {Color::rgb(26,112,77)}else{Color::WHITE});
        }
        if !overlay {expected.text(&mut font,b"green: pointer down / white: up",0,384,400,1,Color::rgb(161,183,204));}
    }
    expected.compare(d,bytes)
}
fn check_nonpicture(s: &Show, chosen: usize, bytes: &[u8], d: Dims) -> Result<(), String> {
    let entry=&s.entries[chosen];
    if entry.timing.is_some() { return Ok(()); } // timing has its full literal guest fixture
    if let Some(input)=&entry.input { return check_input(bytes,input,false,d); }
    if let Some(value)=&entry.reading {
        let mut font=Oracle::new();
        for (top,height,scale,text) in [
            (208,72,2,value.name.clone()),
            (280,90,3,std::format!("{} {}",value.number,value.unit)),
            (392,90,2,match value.expect {
                Some((lo,hi)) if lo==hi=>std::format!("Expected exactly {lo} {}",value.unit),
                Some((lo,hi))=>std::format!("Bound: {lo} to {hi} {}",value.unit),
                None=>"No numeric bound supplied".into(),
            }),
        ] {
            let mut expected=Region::new(d,848,top,400,height,Color::rgb(18,29,44))?;
            words(&mut expected,&mut font,&text,0,0,400,scale);
            expected.compare(d,bytes)?;
        }
    }
    Ok(())
}
fn check_labels(s: &Show, bytes: &[u8], d: Dims) -> Result<(), String> {
    let ids:Vec<_>=s.entries.iter().map(|e|Tile{id:&e.id,stamp:e.stamp,key:e.key,thumbnail:None}).collect();
    let wall=stage::Wall::for_tiles(&ids).map_err(|e|std::format!("wall: {e:?}"))?;
    let mut font=Oracle::new();
    for (id,first,end) in &s.groups {
        let (x,y)=wall.origin(wall.slot(&ids,*first)-2);
        let mut expected=Region::new(d,x,y,78*wall.scale,24*wall.scale,Color::rgb(24,39,59))?;
        let passed=s.entries[*first..*end].iter().filter(|e|e.stamp==Stamp::Pass).count();
        expected.text(&mut font,id.as_bytes(),2,2,76*wall.scale,wall.scale,Color::WHITE);
        expected.text(&mut font,std::format!("{passed}/{} pass",end-first).as_bytes(),2,14*wall.scale,76*wall.scale,wall.scale,Color::WHITE);
        expected.compare(d,bytes)?;
    }
    let entry=&s.entries[s.selected];
    let (label,colour)=match entry.stamp {
        Stamp::Pass=>("PASS",Color::rgb(26,112,77)),
        Stamp::Fail=>("FAIL",Color::rgb(145,50,58)),
        Stamp::NotReached=>("NOT REACHED",Color::rgb(57,67,80)),
        _=>return Err("label check needs a completed case".into()),
    };
    let rows=font.rows(d.n,3);
    let mut expected=Region::new(d,848,672,400,rows,Color::rgb(18,29,44))?;
    expected.text(&mut font,label.as_bytes(),0,0,400,3,colour);
    expected.compare(d,bytes)?;
    Region::new(d,0,700,1280,100,Color::rgb(12,19,31))?.compare(d,bytes)?;
    Ok(())
}
fn exercise_details(s: &Show) -> CaseResult {
    let mut probe=s.clone();probe.finale=None;probe.first_failure=None;probe.last=None;
    let selected=probe.selected;
    probe.entries[selected].timing=None;
    probe.entries[selected].stamp=Stamp::Pass;
    for kind in 0..3 {
        probe.entries[selected].input = if kind==0 {None} else {Some(super::super::suite::InputTrace{
            points: if kind==1 {Vec::new()}else{std::vec![stage::InputPoint{x:10,y:20,pressed:false},stage::InputPoint{x:30,y:40,pressed:true}]},
            hit:"button OK: Clicked".into(),
        })};
        probe.entries[selected].reading=Some(super::super::suite::Value{name:"pixels checked".into(),number:128,unit:"px".into(),expect:Some((128,128))});
        for faulty in [true,false] {
            if faulty {crate::test_control::arm(13,process::getpid()?.raw() as u64,0)?;}
            probe.draw(fault_bits()).map_err(crate::suite::CaseError::Fail)?;
            let (mut bytes,seen)=snapshot().map_err(crate::suite::CaseError::Fail)?;
            let d=Dims::of(&seen);
            let held=remote::drawn().map_err(crate::suite::CaseError::Fail)?;
            check(check_spotlight(&probe,&bytes,d,&held).is_err()==faulty,"input/measurement substitution escaped its own check")?;
            if faulty {check(crate::test_control::fired(13)?>0,"spotlight fault did not fire")?;}
            crate::test_control::disarm_all()?;
            if !faulty {
                check_labels(&probe,&bytes,d).map_err(crate::suite::CaseError::Fail)?;
                let tiles:Vec<_>=probe.entries.iter().map(|e|Tile{id:&e.id,stamp:e.stamp,key:e.key,thumbnail:None}).collect();
                let wall=stage::Wall::for_tiles(&tiles).map_err(|e|std::format!("wall: {e:?}"))?;
                let (x,y)=wall.origin(wall.slot(&tiles,0)-2);
                for (x,y) in [(x,y),(848,672),(16,728),(16,752)] {
                    bytes[d.at(x,y)]^=255;let caught=check_labels(&probe,&bytes,d).is_err();bytes[d.at(x,y)]^=255;
                    check(caught,"heading, verdict or empty-footer substitution escaped detection")?;
                }
            }
        }
    }
    // A picture with both timing data and input must retain its picture and input;
    // timing is shown only when no picture is present.
    let pixels=std::vec![10,20,30,255];let hash=libpixcheck::sha256(&pixels);
    probe.entries[selected].line=std::format!("sha256={}",hash.iter().map(|b|std::format!("{b:02x}")).collect::<String>());
    probe.last=Some(Arc::new(JudgedFrame{width:1,height:1,pixels,sha256:hash,expected:Some(std::vec![10,20,30,255]),bound:0}));
    probe.entries[selected].published=true;
    probe.entries[selected].timing=Some(FrameTiming{frame_us:std::vec![1000],budget_us:1000});
    probe.draw(0).map_err(crate::suite::CaseError::Fail)?;
    let (mut bytes,seen)=snapshot().map_err(crate::suite::CaseError::Fail)?;
    let d=Dims::of(&seen);
    let held=remote::drawn().map_err(crate::suite::CaseError::Fail)?;
    check(held.panes(&libpixcheck::sha256(&[10,20,30,255])).is_some(),"the runner did not draw the check's own picture")?;
    check_spotlight(&probe,&bytes,d,&held).map_err(crate::suite::CaseError::Fail)?;
    for (x,y) in [(848,448),(862,514),(1040,556)] {
        bytes[d.at(x,y)]^=255;let caught=check_spotlight(&probe,&bytes,d,&held).is_err();bytes[d.at(x,y)]^=255;
        check(caught,"picture input overlay substitution escaped detection")?;
    }
    Ok(())
}
