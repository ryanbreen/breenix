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
        let mut fb=FrameBuf::new(1280,800,Format::BGRX).map_err(|e|e.to_string())?;
        final_show.render(&mut fb,0).map_err(crate::suite::CaseError::Fail)?;
        check_spotlight(&final_show,&fb_bytes(&fb)).map_err(crate::suite::CaseError::Fail)?;
        let mut f=JudgedFrame{width:1,height:1,pixels:std::vec![10,20,30,255],sha256:[0;32],expected:Some(std::vec![10,25,30,255]),bound:0};
        f.sha256=libpixcheck::sha256(&f.pixels);
        let mut probe=s.clone();probe.first_failure=None;probe.finale=None;
        probe.entries[probe.selected].timing=None;probe.entries[probe.selected].input=None;
        probe.entries[probe.selected].line=std::format!(" sha256={}",f.sha256.iter().map(|b|std::format!("{b:02x}")).collect::<String>());
        probe.last=Some(Arc::new(f));probe.draw(0).map_err(crate::suite::CaseError::Fail)?;
        let (px,_)=snapshot().map_err(crate::suite::CaseError::Fail)?;
        check_spotlight(&probe,&px).map_err(crate::suite::CaseError::Fail)?;
        for (x,y) in [(840,168),(984,168),(1128,168),(1136,176),(840,312)] {
            let mut wrong=px.clone();wrong[(y*1280+x)*4]^=255;
            check(check_spotlight(&probe,&wrong).is_err(),"spotlight accepted a substituted pane, heat map, cross or bound")?;
        }
    }
    let point = match name {
        "judged-frame-shown" => Some(13),
        "serial-agreement" => Some(12),
        "boundary-notice" => None,
        _ => return Err("unknown live stage check".into()),
    };
    let ids:Vec<_>=s.entries.iter().map(|e|e.id.clone()).collect();
    let tiles:Vec<_>=ids.iter().map(|id|Tile{id,stamp:Stamp::Pending,key:0,thumbnail:None}).collect();
    let wall=stage::Wall::for_tiles(&tiles);
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
        let mut mismatch = false;
        match name {
            "judged-frame-shown" => {
                let mut n = 0;
                for (i, e) in s.entries.iter().enumerate() {
                    if let Some(p) = &e.picture {
                        n += 1;
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
                        let (left,top)=(x0+2*wall.scale,y0+2*wall.scale);
                        for y in 0..p.height*wall.scale {
                            for x in 0..p.width*wall.scale {
                                let o = ((top + y) * 1280 + left + x) * 4;
                                let r = ((y/wall.scale) * p.width + x/wall.scale) * 4;
                                mismatch |= pixels[o..o + 4] != p.pixels[r..r + 4];
                            }
                        }
                    }
                }
                check(n > 0, "no earlier judged pictures to check")?;
                // Independently compute every displayed source pixel, heat-map channel,
                // first-mismatch cross and caption; a correct tile cannot conceal a false spotlight.
                mismatch |= check_spotlight(&s,&pixels).is_err();
            }
            "serial-agreement" => {
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
                        Some("PASS") => libgfx::color::Color::rgb(26, 112, 77),
                        Some("FAIL") => libgfx::color::Color::rgb(145, 50, 58),
                        Some("SKIP") => libgfx::color::Color::rgb(57, 67, 80),
                        _ => return Err("invalid stored CASE verdict".into()),
                    };
                    let mut expected = FrameBuf::new_filled(
                        38*scale,
                        24*scale,
                        Format::BGRX,
                        libgfx::color::Color::rgb(12, 19, 31),
                    )
                    .map_err(|e| e.to_string())?;
                    libgfx::shapes::draw_rect(&mut expected, 0, 0, (38*scale) as i32, (24*scale) as i32, colour);
                    libgfx::shapes::fill_rect(
                        &mut expected,
                        (2*scale) as i32,
                        (2*scale) as i32,
                        (16*scale) as i32,
                        (12*scale) as i32,
                        libgfx::color::Color::rgb(18, 29, 44),
                    );
                    if let Some(p) = &e.picture {
                        for y in 0..p.height*scale {
                            for x in 0..p.width*scale {
                                let o = ((y/scale) * p.width + x/scale) * 4;
                                expected.put_pixel(
                                    x + 2*scale,
                                    y + 2*scale,
                                    libgfx::color::Color::rgb(
                                        p.pixels[o + 2],
                                        p.pixels[o + 1],
                                        p.pixels[o],
                                    ),
                                );
                            }
                        }
                    }
                    expected.push_clip(libgfx::framebuf::Rect::new((20*scale) as i32,(2*scale) as i32,(18*scale) as i32,(7*scale) as i32));
                    libgfx::font::draw_text(
                        &mut expected,
                        std::format!("{}", i + 1).as_bytes(),
                        20*scale,
                        2*scale,
                        libgfx::color::Color::WHITE,
                        scale,
                    );
                    expected.pop_clip();
                    libgfx::shapes::fill_rect(&mut expected, 1, (15*scale) as i32, (38*scale-2) as i32, (9*scale-1) as i32, colour);
                    let number = words
                        .iter()
                        .find_map(|w| w.strip_prefix("ms="))
                        .unwrap_or("0");
                    let n = number
                        .parse::<u64>()
                        .map_err(|_| "invalid CASE milliseconds")?;
                    let key = if n > 99999 { ">" } else { number };
                    expected.push_clip(libgfx::framebuf::Rect::new((2*scale) as i32,(16*scale) as i32,(34*scale) as i32,(7*scale) as i32));
                    libgfx::font::draw_text(
                        &mut expected,
                        key.as_bytes(),
                        2*scale,
                        16*scale,
                        libgfx::color::Color::WHITE,
                        scale,
                    );
                    expected.pop_clip();
                    for y in 0..24*scale {
                        for x in 0..38*scale {
                            let o = ((top + y) * 1280 + left + x) * 4;
                            let p = expected.get_pixel(x, y);
                            mismatch |= pixels[o..o + 4] != [p.b, p.g, p.r, 255];
                        }
                    }
                }
            }
            "boundary-notice" => {
                let notice = match (rec.backend, rec.flags, rec.boundary) {
                    (1, 1, 1) => "host copy and window outside guest view",
                    (2, 0, 2) => "host scanning outside guest view",
                    (3 | 4, 0, 3) => "host window outside guest view",
                    _ => return Err("invalid observation boundary combination".into()),
                };
                let mut expected = FrameBuf::new_filled(
                    1280,
                    800,
                    Format::BGRX,
                    libgfx::color::Color::rgb(12, 19, 31),
                )
                .map_err(|e| e.to_string())?;
                libgfx::font::draw_text(
                    &mut expected,
                    notice.as_bytes(),
                    24,
                    776,
                    libgfx::color::Color::rgb(161, 183, 204),
                    1,
                );
                for y in 776..783 {
                    for x in 24..1280 {
                        let o = (y * 1280 + x) * 4;
                        let p = expected.get_pixel(x, y);
                        mismatch |= pixels[o..o + 4] != [p.b, p.g, p.r, 255];
                    }
                }
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

fn fb_bytes(fb:&FrameBuf)->Vec<u8> {
    let mut bytes=Vec::with_capacity(1280*800*4);
    for y in 0..800 {for x in 0..1280 {let p=fb.get_pixel(x,y);bytes.extend_from_slice(&[p.b,p.g,p.r,255]);}}
    bytes
}
/// Literal spotlight formulas from section 23, applied to the verified retained frame.
fn check_spotlight(s:&Show,bytes:&[u8])->Result<(),String> {
    let chosen=s.first_failure.as_ref().filter(|_|s.finale.is_some()).map_or(s.selected,|v|v.0);
    let frame=if s.finale.is_some() && s.first_failure.is_some(){s.first_failure.as_ref().and_then(|v|v.1.as_deref())}else{s.last.as_deref()}.ok_or("no judged spotlight source")?;
    verify_source(&s.entries[chosen].line,frame)?;
    let pixel=|x:usize,y:usize| &bytes[(y*1280+x)*4..][..4];
    if let Some(expected)=&frame.expected {
        let mut first=None;let mut differing=0;
        for (i,(a,b)) in frame.pixels.chunks_exact(4).zip(expected.chunks_exact(4)).enumerate(){if a!=b{differing+=1;first.get_or_insert((i%frame.width,i/frame.width));}}
        for y in 0..128 {for x in 0..128 {
            let i=((y*frame.height/128)*frame.width+x*frame.width/128)*4;
            let a=&frame.pixels[i..i+4];let e=&expected[i..i+4];
            let d=(0..3).map(|c|a[c].abs_diff(e[c])).max().unwrap();
            let cross=first.is_some_and(|(fx,fy)|{let (cx,cy)=(fx*128/frame.width,fy*128/frame.height);(y==cy && x.abs_diff(cx)<=2)||(x==cx && y.abs_diff(cy)<=2)});
            let diff=if cross{[255,255,255,255]}else if d==0{[0,0,0,255]}else{[0,255-d,255,255]};
            if pixel(840+x,168+y)!=e || pixel(984+x,168+y)!=a || pixel(1128+x,168+y)!=diff{return Err("spotlight differs from judged actual, reference or heat map".into());}
        }}
        let mut label=FrameBuf::new_filled(424,7,Format::BGRX,libgfx::color::Color::rgb(18,29,44)).map_err(|e|e.to_string())?;
        libgfx::font::draw_text(&mut label,std::format!("{differing} pixels differ; bound {}",frame.bound).as_bytes(),0,0,libgfx::color::Color::WHITE,1);
        for y in 0..7 {for x in 0..424 {let p=label.get_pixel(x,y);if pixel(840+x,312+y)!=[p.b,p.g,p.r,255]{return Err("spotlight bound differs".into());}}}
    }else{
        for y in 0..250 {for x in 0..400 {
            let i=((y*frame.height/250)*frame.width+x*frame.width/400)*4;
            if pixel(848+x,168+y)!=&frame.pixels[i..i+4]{return Err("actual-only spotlight differs from judged source".into());}
        }}
    }
    Ok(())
}
