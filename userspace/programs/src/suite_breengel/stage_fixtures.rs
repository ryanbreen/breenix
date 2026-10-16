//! Literal section 23 fixtures shared by the guest cases and the host developer loop.
//! Expected bytes come only from contract rectangles, colours and formulas.
use super::stage::{self, Finale, Image, Milestone, Run, Spotlight, Stamp, Tile};
use libgfx::{color::Color, font, framebuf::FrameBuf};

fn surface(bytes: &mut [u8]) -> FrameBuf {
    unsafe { FrameBuf::from_raw(bytes.as_mut_ptr(),1280,800,5120,4,true).unwrap() }
}
fn expect_pixel(fb:&FrameBuf,x:usize,y:usize,rgb:(u8,u8,u8)) -> Result<(),String> {
    let p=fb.get_pixel(x,y);
    if (p.r,p.g,p.b)==rgb { Ok(()) } else { Err(format!("pixel {x},{y}: got {},{},{} expected {rgb:?}",p.r,p.g,p.b)) }
}
fn rect(fb:&FrameBuf,x:usize,y:usize,w:usize,h:usize,rgb:(u8,u8,u8)) -> Result<(),String> {
    for py in y..y+h { for px in x..x+w { expect_pixel(fb,px,py,rgb)?; } } Ok(())
}
fn text(fb:&FrameBuf,x:usize,y:usize,s:&str,scale:usize,background:(u8,u8,u8)) -> Result<(),String> {
    let w=font::text_width(s.as_bytes(),scale); let h=7*scale;
    let mut bytes=vec![0u8;1280*800*4]; let mut reference=surface(&mut bytes);
    for py in y..y+h { for px in x..x+w { reference.put_pixel(px,py,Color::rgb(background.0,background.1,background.2)); } }
    font::draw_text(&mut reference,s.as_bytes(),x,y,Color::WHITE,scale);
    for py in y..y+h { for px in x..x+w {
        let p=reference.get_pixel(px,py); expect_pixel(fb,px,py,(p.r,p.g,p.b))?;
    } } Ok(())
}
fn run<'a>(tiles:&'a [Tile<'a>],milestones:&'a [Milestone<'a>]) -> Run<'a> {
    Run { suite:"fixture",target:"qemu",case_number:3,ui_time_ms:250,fps:60,tiles,milestones,selected:1,
        spotlight:Spotlight::None,finale:None,boundary:"host copy and window outside guest view" }
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

pub fn check(name:&str) -> Result<(),String> {
    let mut bytes=vec![0u8;1280*800*4]; let mut fb=surface(&mut bytes);
    let mut cases=tiles(); let mut model=run(&cases,&MILESTONES);
    match name {
        "stage-layout" => {
            render(&mut fb,&model)?;
            rect(&fb,0,0,1280,16,(24,39,59))?;
            rect(&fb,0,96,1280,16,(12,19,31))?;
            rect(&fb,832,112,432,8,(18,29,44))?;
            rect(&fb,16,712,1248,16,(12,19,31))?;
            text(&fb,24,16,"BREENIX / fixture",3,(24,39,59))?;
            text(&fb,24,48,"qemu  case 3 of 5  elapsed 250 ms  fps 60",1,(24,39,59))?;
            text(&fb,24,68,"1 passed  1 failed  1 not reached",2,(24,39,59))?;
            text(&fb,840,120,"CASE 2 / alpha",1,(18,29,44))?;
            text(&fb,840,132,"fail",1,(18,29,44))
        }
        "tile-stamps" => {
            render(&mut fb,&model)?;
            // Slot 2 is the gap between milestones, so beta starts in slot 3.
            for (slot,key,color) in [(0,7,(26,112,77)),(1,9,(145,50,58)),(3,0,(57,67,80)),(5,12,(64,191,224))] {
                let x=16+slot*40;
                rect(&fb,x,112,38,1,color)?; rect(&fb,x+1,127,36,2,color)?;
                text(&fb,x+2,129,&format!("{key}"),1,color)?;
            }
            rect(&fb,96,112,38,26,(12,19,31))?;
            rect(&fb,176,112,38,1,(48,67,96))?;
            rect(&fb,177,127,36,10,(12,19,31))?;
            for (slot,ordinal) in [(0,1),(1,2),(3,3),(4,4),(5,5)] {
                text(&fb,16+slot*40+20,114,&format!("{ordinal}"),1,(12,19,31))?;
            } Ok(())
        }
        "tile-thumbnail" => {
            // Every 2x2 bin has four different pixels, so sampling cannot pass.
            let mut pixels=vec![0u8;32*24*4];
            for y in 0..24 { for x in 0..32 {
                let o=(y*32+x)*4; pixels[o..o+4].copy_from_slice(&[x as u8,y as u8,(x+y) as u8,255]);
            } }
            cases[0].thumbnail=Some(Image{pixels:&pixels,width:32,height:24});
            let model=run(&cases,&MILESTONES);render(&mut fb,&model)?;
            for y in 0..12 { for x in 0..16 {
                expect_pixel(&fb,18+x,114+y,((2*x+2*y+1) as u8,(2*y+1) as u8,(2*x+1) as u8))?;
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
            text(&fb,840,312,"1 pixels differ; bound 0",1,(18,29,44))
        }
        "spotlight-timing" => {
            let times=[0,500,1000,2000];
            model.spotlight=Spotlight::Timing{frame_us:&times,budget_us:1000};render(&mut fb,&model)?;
            rect(&fb,848,336,99,96,(18,29,44))?;
            rect(&fb,848,433,99,95,(18,29,44))?;
            rect(&fb,948,480,99,48,(64,160,224))?;
            rect(&fb,1048,433,99,95,(64,160,224))?;
            rect(&fb,1148,336,99,96,(145,50,58))?;
            rect(&fb,848,432,400,1,(255,255,255))?;
            text(&fb,840,312,"frame times us; budget 1000",1,(18,29,44))
        }
        "milestone-ribbon" => {
            cases[1].stamp=Stamp::Pass;
            let model=run(&cases,&MILESTONES);render(&mut fb,&model)?;
            rect(&fb,16,728,622,8,(26,112,77))?;rect(&fb,640,728,622,8,(48,67,96))?;
            rect(&fb,638,728,2,24,(12,19,31))?;
            text(&fb,20,736,"alpha",1,(26,112,77))?;text(&fb,644,736,"beta",1,(48,67,96))
        }
        "finale" => {
            let finale=Finale{started_ms:250,slowest:"alpha/fail",slowest_ms:9};
            if finale.hold_complete(5249)||!finale.hold_complete(5250)||finale.hold_complete(0) { return Err("finale did not hold exactly 5000 ms of UI time".into()); }
            model.finale=Some(finale);render(&mut fb,&model)?;
            rect(&fb,48,240,720,24,(24,39,59))?;
            text(&fb,72,264,"RUN COMPLETE",3,(24,39,59))?;
            text(&fb,72,312,"1 passed  1 failed  1 not reached",2,(24,39,59))?;
            text(&fb,72,352,"slowest alpha/fail 9 ms",1,(24,39,59))
        }
        "deterministic" => {
            bytes.fill(0x35);
            let mut fb=surface(&mut bytes);render(&mut fb,&model)?;let before=bytes.clone();
            std::thread::sleep(std::time::Duration::from_millis(20));
            bytes.fill(0xCA);
            let mut fb=surface(&mut bytes);render(&mut fb,&model)?;
            if bytes!=before { Err("same results and UI time drew different stage bytes".into()) } else { Ok(()) }
        }
        _ => Err(format!("unknown stage fixture {name}")),
    }
}
