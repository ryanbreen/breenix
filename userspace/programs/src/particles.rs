//! Bouncing particle physics in a compositor window.
use breengel::{Event, Handled, Window};
use libgfx::{color::Color, shapes};
#[path = "particles/sim.rs"] mod sim;
#[path = "demos/status.rs"] mod status;

fn main() {
    let mut win = Window::new_without_fonts(b"Particles", 400, 300).expect("window");
    let mut sim = sim::Simulation::new(400, 300);
    let mut last = None;
    let mut redraw = true;
    win.set_animating(true);
    loop {
        if !redraw {
            let mut close = false; let mut resize = None; let mut input = false;
            let count = win.wait_dispatch(100_000_000, |d| {
                input |= d.serial != 0;
                match d.event {
                    Event::AnimationTick { time_ms } => { sim.advance(last.map_or(1, |t: u64| time_ms.saturating_sub(t))); last = Some(time_ms); redraw = true; }
                    Event::CloseRequested => close = true,
                    Event::ResizeRequested { width, height } => { resize = Some((width, height)); redraw = true; }
                    Event::Resized { width, height } => { sim.resize(width as i32, height as i32); redraw = true; }
                    _ => {}
                }
                if redraw { Handled::Redraw } else { Handled::NoChange }
            }).expect("dispatch");
            if close { return; }
            if let Some((w,h)) = resize { if win.apply_resize(w,h).is_ok() { sim.resize(w as i32,h as i32); } }
            if !redraw { if count != 0 && input { let _ = win.commit_no_change(); } continue; }
        }
        redraw = false;
        let objects: Vec<_> = sim.particles.iter().map(|p| status::Object { bounds: [(p.px()-p.radius) as i64, (p.py()-p.radius) as i64, (p.radius*2) as i64, (p.radius*2) as i64], value: serde_json::json!({"vx":p.vx,"vy":p.vy}) }).collect();
        win.set_tree(status::build("particles",100,sim.epochs(),sim.all_inside(),win.width(),win.height(),"particle",&objects));
        let fb = win.surface().expect("surface");
        fb.clear(Color::rgb(15,20,35));
        for p in &sim.particles { shapes::fill_circle(fb,p.px(),p.py(),p.radius,Color::rgb(p.color.r,p.color.g,p.color.b)); if p.radius > 6 { shapes::fill_circle(fb,p.px()-p.radius/3,p.py()-p.radius/3,(p.radius/4).max(2),Color::WHITE); } }
        win.present().expect("present");
    }
}
