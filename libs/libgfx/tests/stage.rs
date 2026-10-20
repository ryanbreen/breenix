// Exercise DISPLAY's module before RASTER exports it from libgfx's shared root.
use libgfx::{color,font,framebuf,shapes};
#[path="../src/stage.rs"]
pub mod stage;
#[path="../../../userspace/programs/src/suite_breengel/stage_fixtures.rs"]
mod fixtures;
#[test] fn stage_layout(){fixtures::check("stage-layout").unwrap();}
#[test] fn tile_stamps(){fixtures::check("tile-stamps").unwrap();}
#[test] fn tile_thumbnail(){fixtures::check("tile-thumbnail").unwrap();}
#[test] fn spotlight_picture(){fixtures::check("spotlight-picture").unwrap();}
#[test] fn spotlight_timing(){fixtures::check("spotlight-timing").unwrap();}
#[test] fn spotlight_input(){fixtures::check("spotlight-input").unwrap();}
#[test] fn spotlight_measurement(){fixtures::check("spotlight-measurement").unwrap();}
#[test] fn milestone_ribbon(){fixtures::check("milestone-ribbon").unwrap();}
#[test] fn finale(){fixtures::check("finale").unwrap();}
#[test] fn deterministic(){fixtures::check("deterministic").unwrap();}

#[test]
fn rejects_spotlight_statistics_that_disagree_with_pixels() {
    use stage::*;
    let image = [0,0,0,255];
    let tiles = [Tile{id:"a/b",stamp:Stamp::Fail,key:0,thumbnail:None}];
    let milestones = [Milestone{id:"a",first:0,end:1}];
    let run = Run{suite:"x",target:"qemu",case_number:1,ui_time_ms:0,fps:0,tiles:&tiles,
        milestones:&milestones,selected:0,input:None,spotlight:Spotlight::Picture{
        expected:Image{pixels:&image,width:1,height:1},actual:Image{pixels:&image,width:1,height:1},
        first:Some((0,0)),differing:1,bound:0},finale:None,boundary:stage::Observation{backend:1,flags:1,boundary:1}};
    let mut fb=framebuf::FrameBuf::new(1280,800,framebuf::Format::BGRX).unwrap();
    assert_eq!(draw(&mut fb,&run),Err(StageError::Image));
}

#[test] fn stage_hidpi(){fixtures::check("stage-hidpi").unwrap();}
