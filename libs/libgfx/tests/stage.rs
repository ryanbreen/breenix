// Exercise DISPLAY's module before RASTER exports it from libgfx's shared root.
use libgfx::{color,framebuf};
#[path="../src/stage.rs"]
pub mod stage;
#[path="../../../userspace/programs/src/suite_breengel/stage_fixtures.rs"]
mod fixtures;
#[test] fn stage_layout(){fixtures::check("stage-layout").unwrap();}
#[test] fn tile_stamps(){fixtures::check("tile-stamps").unwrap();}
#[test] fn tile_thumbnail(){fixtures::check("tile-thumbnail").unwrap();}
#[test] fn spotlight_picture(){fixtures::check("spotlight-picture").unwrap();}
#[test] fn spotlight_timing(){fixtures::check("spotlight-timing").unwrap();}
#[test] fn milestone_ribbon(){fixtures::check("milestone-ribbon").unwrap();}
#[test] fn finale(){fixtures::check("finale").unwrap();}
#[test] fn deterministic(){fixtures::check("deterministic").unwrap();}
