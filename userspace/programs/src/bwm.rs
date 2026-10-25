//! Breenix Window Manager (bwm) — Pure Compositor + Input Router
//!
//! Discovers Breengel client windows registered on the window interface (version 2,
//! `docs/gui/breengel.md` section 17), composes them with its own desktop on the GPU core
//! (`compositor`, ruling B13), and routes keyboard and pointer input to the focused window
//! as window events (section 19).
//!
//! BWM does not emulate terminals; init starts the session's programs, and bwm starts only
//! the ones its hotkeys name.

#[path = "bwm/compositor.rs"]
mod compositor;
#[path = "bwm/input_backlog.rs"]
mod input_backlog;
#[path = "bwm/input.rs"]
mod input;
use input::*;
#[path = "bwm/placement.rs"]
mod placement;
#[path = "bwm/session.rs"]
mod session;

use std::process;

use compositor::{CheckError, Compositor, FrameError, Layer, Pointer};
use session::{cursor_shape, Input};

use libbreenix::fs;
use libbreenix::io;
use libbreenix::signal;

use breengel::FontConfig;
use libgfx::bitmap_font;
use libgfx::color::Color;
use libgfx::surface::{Rect as Clip, Surface};
use libgfx::ttf_font;
use libgfx::ttf_font::CachedFont;
use libgfx::ttf_font::Font;

#[path = "bwm/clock.rs"]
mod clock;

#[path = "bwm/config.rs"]
mod config;
use config::*;
#[path = "bwm/route.rs"]
mod route;
use route::*;
#[path = "bwm/geometry.rs"]
mod geometry;
use geometry::*;
#[path = "bwm/desk.rs"]
mod desk;
use desk::*;
#[path = "bwm/chrome.rs"]
mod chrome;
use chrome::*;
#[path = "bwm/launch.rs"]
mod launch;
use launch::*;

fn main() {
    let mut wm = config::start();
    loop {
        let (input_ready, scene_changed) = wm.wait();
        wm.begin_epoch();
        wm.discover(scene_changed);
        wm.chromeless_order();
        let mut damage = Damage::new();
        wm.input(input_ready, &mut damage);
        wm.clock(&mut damage);
        wm.present(damage);
        wm.acknowledge();
    }
}
