//! Literal section 23 fixtures shared by the guest cases and the host developer loop.
//! Expected bytes come only from contract rectangles, colours and formulas.
use super::stage::{self, Finale, Image, Milestone, Run, Spotlight, Stamp, Tile};
use libgfx_ref::{color::Color, framebuf::FrameBuf};
use libgfx_ref::ttf_font::{CachedFont,Font};
thread_local! {static DISPLAY_SCALE:std::cell::Cell<usize>=const{std::cell::Cell::new(1)};}

// claim-lint:ok: docs/gui/breengel.md specifies this assertion; the GPU lanes supply its implementation.
fn surface() -> FrameBuf {
    let n=DISPLAY_SCALE.with(|s|s.get());
    FrameBuf::new(1280*n, 800*n, libgfx_ref::framebuf::Format::BGRX).unwrap()
}
fn expect_pixel(fb:&FrameBuf,x:usize,y:usize,rgb:(u8,u8,u8)) -> Result<(),String> {
    let n=fb.width/1280;
    for dy in 0..n {for dx in 0..n {let p=fb.get_pixel(x*n+dx,y*n+dy);
        if (p.r,p.g,p.b)!=rgb {return Err(format!("pixel {},{}: got {},{},{} expected {rgb:?}",x*n+dx,y*n+dy,p.r,p.g,p.b));}
    }}Ok(())
}
fn expected_pixels(fb:&FrameBuf,want:&FrameBuf,x:usize,y:usize)->Result<(),String>{
    let n=fb.width/1280;
    for dy in 0..n {for dx in 0..n {let (x,y)=(x*n+dx,y*n+dy);let (p,q)=(fb.get_pixel(x,y),want.get_pixel(x,y));
        if p!=q {return Err(format!("gpu-display: stage pixel {x},{y} differs from the certified reference"));}
    }}Ok(())
}
fn put(fb:&mut FrameBuf,x:usize,y:usize,c:Color){let n=fb.width/1280;for dy in 0..n {for dx in 0..n {fb.put_pixel(x*n+dx,y*n+dy,c);}}}
fn oracle_fill(fb:&mut FrameBuf,x:i32,y:i32,w:i32,h:i32,c:Color){let n=(fb.width/1280) as i32;libgfx_ref::shapes::fill_rect(fb,x*n,y*n,w*n,h*n,c);}
/// Lower-layer §8.3 text oracle supplies the stage expectation independently.
fn oracle_text(fb:&mut FrameBuf,t:&[u8],x:usize,y:usize,c:Color,legacy_scale:usize){
    let n=fb.width/1280;
    if n==1 {libgfx_ref::font::draw_text(fb,t,x,y,c,legacy_scale);return;}
    let font=Font::parse(include_bytes!("../../../../fonts/DejaVuSans.ttf")).expect("pinned DejaVu Sans");
    let mut font=CachedFont::new(font,256);
    libgfx_ref::ttf_font::draw_text_bytes(fb,&mut font,t,(x*n) as i32,(y*n) as i32,(7*legacy_scale*n) as f32,c);
}

fn rect(fb:&FrameBuf,x:usize,y:usize,w:usize,h:usize,rgb:(u8,u8,u8)) -> Result<(),String> {
    for py in y..y+h { for px in x..x+w { expect_pixel(fb,px,py,rgb)?; } } Ok(())
}
fn text(fb:&FrameBuf,x:usize,y:usize,t:&str,scale:usize,background:(u8,u8,u8)) -> Result<(),String> {
    let n=fb.width/1280;
    let (w,h)=if n==1 {(libgfx_ref::font::text_width(t.as_bytes(),scale),7*scale)}else{
        let mut f=CachedFont::new(Font::parse(include_bytes!("../../../../fonts/DejaVuSans.ttf")).map_err(|e|e.to_string())?,256);
        let size=(7*scale*n) as f32;let m=f.metrics(size);
        ((libgfx_ref::ttf_font::text_width(&mut f,t,size) as usize).div_ceil(n),((m.ascender-m.descender+m.line_gap).ceil() as usize).div_ceil(n))
    };
    let mut reference=surface();
    for py in y..y+h {for px in x..(x+w).min(1280) {put(&mut reference,px,py,Color::rgb(background.0,background.1,background.2));}}
    oracle_text(&mut reference,t.as_bytes(),x,y,Color::WHITE,scale);
    for py in y..y+h {for px in x..(x+w).min(1280) {expected_pixels(fb,&reference,px,py)?;}}Ok(())
}
/// `text` at legacy `scale`, for text inside a box whose bottom edge is logical row `bottom`
/// (rulings B25, B35): the oracle is drawn under the box's clip and only the box's rows are compared.
fn text_in_box(fb:&FrameBuf,x:usize,y:usize,t:&str,scale:usize,bottom:usize,background:(u8,u8,u8)) -> Result<(),String> {
    let n=fb.width/1280;
    let mut f=CachedFont::new(Font::parse(include_bytes!("../../../../fonts/DejaVuSans.ttf")).map_err(|e|e.to_string())?,256);
    let size=(7*scale*n) as f32;
    let w=(libgfx_ref::ttf_font::text_width(&mut f,t,size) as usize).div_ceil(n);
    let mut reference=surface();
    for py in y..bottom {for px in x..(x+w).min(1280) {put(&mut reference,px,py,Color::rgb(background.0,background.1,background.2));}}
    reference.push_clip(libgfx_ref::framebuf::Rect::new(0,(y*n) as i32,(1280*n) as i32,((bottom-y)*n) as i32));
    oracle_text(&mut reference,t.as_bytes(),x,y,Color::WHITE,scale);
    reference.pop_clip();
    for py in y..bottom {for px in x..(x+w).min(1280) {expected_pixels(fb,&reference,px,py)?;}}Ok(())
}

fn run<'a>(tiles:&'a [Tile<'a>],milestones:&'a [Milestone<'a>]) -> Run<'a> {
    Run { suite:"fixture",target:"qemu",case_number:3,ui_time_ms:250,fps:60,tiles,milestones,selected:1,
        input:None,spotlight:Spotlight::None,finale:None,boundary:if DISPLAY_SCALE.with(|s|s.get())==2 {stage::Observation{backend:4,flags:3,boundary:4}}else{stage::Observation{backend:1,flags:1,boundary:1}} }
}
fn render(fb:&mut FrameBuf,run:&Run<'_>) -> Result<(),String> {
    stage::draw(fb,run).map_err(|e|format!("missing or invalid run stage (§23): {e:?}"))
}
fn tiles() -> [Tile<'static>;5] {
    [Tile {id:"alpha/pass",stamp:Stamp::Pass,key:7,thumbnail:None},
     Tile {id:"alpha/fail",stamp:Stamp::Fail,key:9,thumbnail:None},
     Tile {id:"beta/not-reached",stamp:Stamp::NotReached,key:0,thumbnail:None},
     Tile {id:"beta/pending",stamp:Stamp::Pending,key:0,thumbnail:None},
     Tile {id:"beta/running",stamp:Stamp::Running,key:12,thumbnail:None}]
}
const MILESTONES:[Milestone<'static>;2]=[Milestone {id:"alpha",first:0,end:2},Milestone{id:"beta",first:2,end:5}];

#[cfg(test)]
pub fn check(name:&str) -> Result<(),String> {check_capture(name,|_|Ok(()))}
pub fn check_capture(name:&str,capture:impl FnOnce(&FrameBuf)->Result<(),String>) -> Result<(),String> {
    let scaled=matches!(name,"stage-layout"|"tile-stamps"|"tile-thumbnail"|"spotlight-picture"|"spotlight-timing"|"spotlight-input"|"spotlight-measurement"|"milestone-ribbon"|"finale"|"stage-hidpi");
    DISPLAY_SCALE.with(|s|s.set(if scaled{2}else{1}));
    struct Reset;impl Drop for Reset{fn drop(&mut self){DISPLAY_SCALE.with(|s|s.set(1));}}let _reset=Reset;
    let mut fb=surface();
    let result=(|| {
    let mut cases=tiles(); let mut model=run(&cases,&MILESTONES);
    match name {
        "stage-hidpi" => {
            render(&mut fb,&model)?;
            if (fb.width,fb.height)!=(2560,1600) {return Err("gpu-display: HiDPI stage does not cover the display".into());}
            rect(&fb,0,799,1280,1,(12,19,31))?;
            text(&fb,24,16,"BREENIX / fixture",3,(24,39,59))?;
            let detailed=(32..90).any(|y|(48..750).any(|x|{
                let p=fb.get_pixel(x,y);let q=fb.get_pixel(x&!1,y&!1);p!=q
            }));
            if !detailed {return Err("gpu-display: stage text is a replicated low-resolution bitmap".into());}
            Ok(())
        }
        "stage-layout" => {
            render(&mut fb,&model)?;
            let mut header=surface();
            oracle_fill(&mut header,0,0,1280,96,Color::rgb(24,39,59));
            for (x,y,t,scale) in [(24,16,"BREENIX / fixture",3),(24,48,"qemu  case 3 of 5  elapsed 250 ms  fps 60",1),(24,68,"1 passed  1 failed  1 not reached",2)] {
                oracle_text(&mut header,t.as_bytes(),x,y,Color::WHITE,scale);
            }
            for y in 0..96 {for x in 0..1280 {expected_pixels(&fb,&header,x,y)?;}}
            rect(&fb,0,0,1280,16,(24,39,59))?;
            rect(&fb,0,96,1280,16,(12,19,31))?;
            rect(&fb,832,112,432,8,(18,29,44))?;
            rect(&fb,16,712,1248,16,(12,19,31))?;
            text(&fb,24,16,"BREENIX / fixture",3,(24,39,59))?;
            text(&fb,24,48,"qemu  case 3 of 5  elapsed 250 ms  fps 60",1,(24,39,59))?;
            text(&fb,24,68,"1 passed  1 failed  1 not reached",2,(24,39,59))?;
            text(&fb,840,120,"CASE 2 / alpha",1,(18,29,44))?;
            text(&fb,840,132,"fail",1,(18,29,44))?;
            text(&fb,840,144,"key 9",1,(18,29,44))?;
            // Five cases expand to 152x96 tiles; the two category headings are visible.
            text(&fb,18,114,"alpha",4,(24,39,59))?;
            text(&fb,18,168,"1/2 pass",4,(24,39,59))?;
            text(&fb,18,218,"beta",4,(24,39,59))?;
            text(&fb,18,272,"0/3 pass",4,(24,39,59))?;
            rect(&fb,16,320,800,380,(12,19,31))?;
            let mut ef=surface();
            for y in 776..783 {for x in 24..1280 {put(&mut ef,x,y,Color::rgb(12,19,31));}}
            oracle_text(&mut ef,b"host renderer and window outside guest view",24,776,Color::rgb(161,183,204),1);
            for y in 776..783 {for x in 24..1280 {expected_pixels(&fb,&ef,x,y)?;}}
            let reached=[true,false,true];
            stage::draw_boot_ribbon(&mut fb,&reached).map_err(|e|format!("{e:?}"))?;
            for (i,lit) in reached.iter().enumerate() {
                let a=16+1248*i/3;let b=16+1248*(i+1)/3;
                rect(&fb,a,752,b-a-2,16,if *lit{(26,112,77)}else{(48,67,96)})?;
                rect(&fb,b-2,752,2,16,(12,19,31))?;
            }
            Ok(())
        }
        "tile-stamps" => {
            render(&mut fb,&model)?;
            // Literal slots: alpha heading 0..2, cases 2,3; slot 4 is a wrap gap;
            // beta heading 5..7, cases 7,8,9. The five tiles are 152x96 at this case count.
            for (x,y,key,color) in [(336,112,7,(26,112,77)),(496,112,9,(145,50,58)),(336,216,0,(57,67,80)),(656,216,12,(64,191,224))] {
                rect(&fb,x,y,152,1,color)?;rect(&fb,x,y+95,152,1,color)?;
                rect(&fb,x,y+1,1,94,color)?;rect(&fb,x+151,y+1,1,94,color)?;
                let mut ef=surface();
                for py in y+60..y+95 {for px in x+1..x+151 {put(&mut ef,px,py,Color::rgb(color.0,color.1,color.2));}}
                oracle_text(&mut ef,format!("{key}").as_bytes(),x+8,y+64,Color::WHITE,4);
                for py in y+60..y+95 {for px in x+1..x+151 {expected_pixels(&fb,&ef,px,py)?;}}
            }
            rect(&fb,656,112,152,96,(12,19,31))?;
            rect(&fb,496,216,152,1,(48,67,96))?;
            rect(&fb,496,311,152,1,(48,67,96))?;
            rect(&fb,496,217,1,94,(48,67,96))?;rect(&fb,647,217,1,94,(48,67,96))?;
            rect(&fb,497,276,150,35,(12,19,31))?;
            for (x,y,ordinal) in [(336,112,1),(496,112,2),(336,216,3),(496,216,4),(656,216,5)] {
                text(&fb,x+80,y+8,&format!("{ordinal}"),4,(12,19,31))?;
            }
            for key in [123456,1024000,u64::MAX] {
                cases[1].key=key;let m=run(&cases,&MILESTONES);render(&mut fb,&m)?;
                // The key text is compared within the tile's own rows, which end at 208 (ruling B35).
                text_in_box(&fb,504,176,">",4,208,(145,50,58))?;
                rect(&fb,528,176,119,28,(145,50,58))?;
                text(&fb,840,144,&format!("key {key}"),1,(18,29,44))?;
            }
            // The complete suite fits without dropping tiles or truncating category names.
            let sizes=[42,38,22,21,9,31,7,24,19,12,40,38,28,32,17,19];
            let ids:Vec<String>=sizes.iter().enumerate().flat_map(|(i,n)|(0..*n).map(move|k|format!("cat{i}/case{k}"))).collect();
            let tiles:Vec<_>=ids.iter().map(|id|Tile{id,stamp:Stamp::Pending,key:0,thumbnail:None}).collect();
            let wall=stage::Wall::for_tiles(&tiles).map_err(|e|format!("wall: {e:?}"))?;
            if tiles.len()!=399 || wall.scale!=1 || wall.columns!=20 {return Err("full wall size or scale changed".into());}
            for i in 0..399 {let (x,y)=wall.origin(wall.slot(&tiles,i));if x+38>816 || y+24>700{return Err(format!("tile {i} is outside wall"));}}
            // Exercise the reported growth: two cases in existing groups and 13 in a new one.
            let mut larger = ids.clone();
            larger.insert(1,"cat0/extra0".into()); larger.insert(2,"cat0/extra1".into());
            larger.extend((0..13).map(|i|format!("cat16/extra{i}")));
            let bigger:Vec<_>=larger.iter().map(|id|Tile{id,stamp:Stamp::Pending,key:0,thumbnail:None}).collect();
            let mut groups=Vec::new();let mut first=0;
            for i in 1..=bigger.len() {
                if i==bigger.len() || bigger[i].id.split('/').next()!=bigger[first].id.split('/').next() {
                    groups.push(Milestone{id:bigger[first].id.split('/').next().unwrap(),first,end:i}); first=i;
                }
            }
            let mut grown=run(&bigger,&groups);grown.selected=0;
            render(&mut fb,&grown)?;
            let packed=stage::Wall::for_tiles(&bigger).map_err(|e|format!("grown wall: {e:?}"))?;
            for i in 0..414 {let (x,y)=packed.origin(packed.slot(&bigger,i));rect(&fb,x,y,38,1,(48,67,96))?;if y+24>700{return Err("grown wall overflow".into());}}
            // A single category also supports more than the retired 420-case cap.
            let many_ids:Vec<_>=(0..430).map(|i|format!("one/{i}")).collect();
            let many:Vec<_>=many_ids.iter().map(|id|Tile{id,stamp:Stamp::Pending,key:0,thumbnail:None}).collect();
            let one=[Milestone{id:"one",first:0,end:430}];let mut big=run(&many,&one);big.selected=0;
            render(&mut fb,&big)?;
            Ok(())
        }
        "tile-thumbnail" => {
            // Each of the 192 2x2 bins of this 32x24 image has four different pixels, so sampling cannot pass.
            let mut pixels=vec![0u8;32*24*4];
            for y in 0..24 { for x in 0..32 {
                let o=(y*32+x)*4; pixels[o..o+4].copy_from_slice(&[x as u8,y as u8,(x+y) as u8,255]);
            } }
            cases[0].thumbnail=Some(Image{pixels:&pixels,width:32,height:24});
            let model=run(&cases,&MILESTONES);render(&mut fb,&model)?;
            for y in 0..12 { for x in 0..16 {
                for yy in 0..4 {for xx in 0..4 {expect_pixel(&fb,344+x*4+xx,120+y*4+yy,((2*x+2*y+1) as u8,(2*y+1) as u8,(2*x+1) as u8))?;}}
            } } Ok(())
        }
        "spotlight-picture" => {
            let expected=[10u8,20,30,255,10,20,30,255,10,20,30,255,10,20,30,255];
            let actual=[10u8,20,30,255,10,25,30,255,10,20,30,255,10,20,30,255];
            model.spotlight=Spotlight::Picture {expected:Image{pixels:&expected,width:2,height:2},actual:Image{pixels:&actual,width:2,height:2},first:Some((1,0)),differing:1,bound:0};
            render(&mut fb,&model)?;
            rect(&fb,840,168,128,128,(30,20,10))?;
            rect(&fb,984,168,64,128,(30,20,10))?;
            rect(&fb,1048,168,64,64,(30,25,10))?;
            rect(&fb,1048,232,64,64,(30,20,10))?;
            rect(&fb,1128,171,64,61,(0,0,0))?;
            rect(&fb,1195,171,61,61,(255,250,0))?;
            for (x,y) in [(1192,168),(1191,168),(1190,168),(1193,168),(1194,168),(1192,169),(1192,170)] { expect_pixel(&fb,x,y,(255,255,255))?; }
            for (x,label) in [(840,"expected"),(984,"actual"),(1128,"difference")] {text(&fb,x,152,label,1,(18,29,44))?;}
            for y in 0..128 {for x in 0..128 {
                let cross=(y==0 && (62..=66).contains(&x)) || (x==64 && y<=2);
                let colour=if cross {(255,255,255)} else if x>=64 && y<64 {(255,250,0)} else {(0,0,0)};
                expect_pixel(&fb,1128+x,168+y,colour)?;
            }}
            text(&fb,840,312,"1 pixels differ; bound 0",1,(18,29,44))?;
            let points=[stage::InputPoint{x:10,y:20,pressed:false},stage::InputPoint{x:30,y:40,pressed:true}];
            model.input=Some((&points,"button OK: Clicked"));
            render(&mut fb,&model)?;
            text(&fb,848,432,"INPUT / pointer trail and handler result",1,(18,29,44))?;
            text(&fb,848,448,"button OK: Clicked",1,(18,29,44))?;
            rect(&fb,862,514,5,5,(255,255,255))?;rect(&fb,1214,594,5,5,(26,112,77))?;
            expect_pixel(&fb,1040,556,(64,160,224))?;
            text(&fb,840,312,"1 pixels differ; bound 0",1,(18,29,44))?;
            model.input=None;
            model.spotlight=Spotlight::Actual(Image{pixels:&actual,width:2,height:2});
            render(&mut fb,&model)?;
            text(&fb,840,152,"actual (reference not supplied)",1,(18,29,44))?;
            for y in 0..250 {for x in 0..400 {
                let i=((y*2/250)*2+x*2/400)*4;
                expect_pixel(&fb,848+x,168+y,(actual[i+2],actual[i+1],actual[i]))?;
            }} Ok(())
        }
        "spotlight-timing" => {
            let times=[0,500,1000,2000];
            model.spotlight=Spotlight::Timing{frame_us:&times,budget_us:1000};render(&mut fb,&model)?;
            for y in 336..528 {for x in 848..1248 {
                let i=(x-848)/100;
                let heights=[0,48,96,192];
                let rgb=if y==432 {(255,255,255)}
                    else if (x-848)%100!=99 && y>=528-heights[i] {
                        if i==3 {(145,50,58)}else{(64,160,224)}
                    }else{(18,29,44)};
                expect_pixel(&fb,x,y,rgb)?;
            }}
            text(&fb,840,312,"frame times us; budget 1000",1,(18,29,44))
        }
        "spotlight-measurement" => {
            model.spotlight=Spotlight::Measurement{name:"pixels checked",number:128,unit:"px",bound:Some((128,128))};
            render(&mut fb,&model)?;
            text(&fb,848,208,"pixels checked",2,(18,29,44))?;
            text(&fb,848,280,"128 px",3,(18,29,44))?;
            text(&fb,848,392,"Expected exactly 128 px",2,(18,29,44))?;
            model.spotlight=Spotlight::Measurement{name:"Case execution time",number:12,unit:"ms",bound:Some((0,1000))};
            render(&mut fb,&model)?;
            text(&fb,848,392,"Bound: 0 to 1000 ms",2,(18,29,44))?;
            Ok(())
        }
        "spotlight-input" => {
            let points=[stage::InputPoint{x:10,y:20,pressed:false},stage::InputPoint{x:30,y:40,pressed:true}];
            model.spotlight=Spotlight::Input{points:&points,hit:"button OK: Clicked"};
            render(&mut fb,&model)?;
            text(&fb,848,216,"button OK: Clicked",2,(18,29,44))?;
            rect(&fb,862,334,5,5,(255,255,255))?;rect(&fb,1214,526,5,5,(26,112,77))?;
            // Interior of the real segment must be blue, not just its two endpoints.
            expect_pixel(&fb,1040,432,(64,160,224))?;
            model.spotlight=Spotlight::Input{points:&[],hit:"checkbox: Space => Toggled(true)"};
            render(&mut fb,&model)?;
            text(&fb,848,360,"Events (no pointer)",2,(18,29,44))?;
            text(&fb,848,184,"INPUT / events",2,(18,29,44))?;
            rect(&fb,848,568,400,7,(18,29,44))?;
            // Whole words move to the next line; the fifth line carries an ellipsis.
            model.spotlight=Spotlight::Input{points:&[],hit:"None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers None Modifiers"};
            render(&mut fb,&model)?;
            text(&fb,848,216,"None Modifiers None Modifiers",2,(18,29,44))?;
            text(&fb,848,234,"None Modifiers None Modifiers",2,(18,29,44))?;
            text(&fb,848,288,"None Modifiers None Modifiers...",2,(18,29,44))?;
            Ok(())
        }
        "milestone-ribbon" => {
            cases[1].stamp=Stamp::Pass;
            let model=run(&cases,&MILESTONES);render(&mut fb,&model)?;
            rect(&fb,16,728,622,8,(26,112,77))?;rect(&fb,640,728,622,8,(48,67,96))?;
            rect(&fb,638,728,2,24,(12,19,31))?;
            rect(&fb,16,743,622,9,(26,112,77))?;
            rect(&fb,640,743,622,9,(48,67,96))?;
            text_in_box(&fb,20,736,"alpha",1,743,(26,112,77))?;text_in_box(&fb,644,736,"beta",1,743,(48,67,96))
        }
        "finale" => {
            let finale=Finale{started_ms:250,slowest:"alpha/fail",slowest_ms:9};
            if finale.hold_complete(5249)||!finale.hold_complete(5250)||finale.hold_complete(0) { return Err("finale did not hold exactly 5000 ms of UI time".into()); }
            model.finale=Some(finale);render(&mut fb,&model)?;
            rect(&fb,48,240,720,24,(24,39,59))?;
            text(&fb,72,264,"RUN COMPLETE",3,(24,39,59))?;
            text(&fb,72,312,"1 passed  1 failed  1 not reached",2,(24,39,59))?;
            text(&fb,72,352,"slowest alpha/fail 9 ms",1,(24,39,59))?;
            let mut ef=surface();
            for y in 240..480 {for x in 48..768 {put(&mut ef,x,y,Color::rgb(24,39,59));}}
            for (x,y,t,scale) in [(72,264,"RUN COMPLETE",3),(72,312,"1 passed  1 failed  1 not reached",2),(72,352,"slowest alpha/fail 9 ms",1)] {
                oracle_text(&mut ef,t.as_bytes(),x,y,Color::WHITE,scale);
            }
            for y in 240..480 {for x in 48..768 {expected_pixels(&fb,&ef,x,y)?;}}
            text(&fb,840,120,"CASE 2 / alpha",1,(18,29,44))?;
            text(&fb,840,132,"fail",1,(18,29,44))
        }
        "deterministic" => {
            let mut other=FrameBuf::new_filled(1280,800,libgfx_ref::framebuf::Format::BGRX,Color::rgb(53,53,53)).map_err(|e|e.to_string())?;
            render(&mut fb,&model)?;
            std::thread::sleep(std::time::Duration::from_millis(20));
            render(&mut other,&model)?;
            for y in 0..800 {for x in 0..1280 {
                let p=other.get_pixel(x,y);expect_pixel(&fb,x,y,(p.r,p.g,p.b))?;
            }} Ok(())
        }
        _ => Err(format!("unknown stage fixture {name}")),
    }})();
    let handed=capture(&fb);
    result.and(handed)
}
