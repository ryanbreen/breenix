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
#[path = "bwm/placement.rs"]
mod placement;
#[path = "bwm/session.rs"]
mod session;

use std::io::Write;
use std::process;

use compositor::{CheckError, Compositor, FrameError, Layer, Pointer};
use session::{cursor_shape, Input};

use libbreenix::fs;
use libbreenix::io;
use libbreenix::signal;
use libbreenix::types::Fd;

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
    launch::initialize();
    let mut wm = config::start();
    loop {
        let (stdin_ready, scene_changed) = wm.wait();
        wm.begin_epoch();
        let (current_mods, super_taps, hotkey_actions) = wm.hotkey_state();
        wm.discover(scene_changed);
        wm.chromeless_order();
        wm.keyboard(stdin_ready, current_mods, super_taps, hotkey_actions);
        let mut damage = Damage::new();
        wm.pointer(&mut damage);
        wm.clock(&mut damage);
        wm.present(damage);
        if !wm.lifecycle() { break; }
        wm.acknowledge();
    }
}
