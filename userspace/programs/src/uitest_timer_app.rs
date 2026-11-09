//! Timer-driven scene for stepped-clock guest cases.
use breengel::{Event, Handled, Window};
use libbreenix::{errno::Errno, error::Error, window::abi};

#[derive(Default)]
struct State {
    ticks: u64,
    position: i64,
    velocity: i64,
    fires: u64,
    missed: u64,
}
impl State {
    fn draw(&self, win: &mut Window) -> Result<(), String> {
        // A pinned old front can occupy the other buffer of this two-buffer
        // window. Record an actual ETIMEDOUT witness before waiting.
        match win.acquire(0) {
            Ok(_) => {}
            Err(Error::Os(Errno::ETIMEDOUT)) => {
                if let Some(root) = std::env::var_os("BREENIX_CONFIG_ROOT") {
                    std::fs::write(std::path::Path::new(&root).join("timer-backpressure"),
                        breengel::clock::epoch().to_string()).map_err(|e| e.to_string())?;
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        let surface = win.surface().map_err(|e| e.to_string())?;
        surface.clear(libgfx::color::Color::rgb((self.ticks % 251) as u8, (self.fires % 251) as u8, 0x60));
        let mut tree = serde_json::json!({"key":"root","parent":null,"role":"window","name":"timer",
            "value":{"ticks":self.ticks,"position":self.position,"velocity":self.velocity,
                "fires":self.fires,"missed":self.missed},
            "bounds":[0,0,320,200],"clip":[0,0,320,200],"flags":["enabled","visible"]}).to_string().into_bytes();
        tree.push(0);
        win.write_tree(&tree).map_err(|e| e.to_string())?;
        win.commit(&[abi::Rect::new(0,0,320,200)]).map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub fn run(title: &str, animate: bool, real_clock: bool) -> Result<(), String> {
    let mut win = Window::new_without_fonts(title.as_bytes(), 320, 200).map_err(|e| e.to_string())?;
    win.timer(1, 1000, 1000);
    if animate {
        win.timer(2, 1, 1);
        win.set_animating(true);
    }
    let mut state = State::default();
    state.draw(&mut win)?;
    libuitest::startup::mark("app-commit");
    loop {
        let mut redraw = false;
        let mut close = false;
        win.wait_dispatch(100_000_000, |delivered| {
            match delivered.event {
                Event::AnimationTick { .. } => {
                    state.ticks += 1;
                    state.velocity += (1000 - state.position) / 16;
                    state.position += state.velocity;
                    state.velocity = state.velocity * 31 / 32;
                    redraw = true;
                }
                Event::Timer { id, missed } => {
                    if id == 1 { state.fires += 1; state.missed = missed; }
                    if real_clock && (id == 2 || !animate) {
                        let _ = breengel::clock::real_now_ns();
                    }
                    redraw = true;
                }
                Event::CloseRequested => close = true,
                _ => {}
            }
            if redraw { Handled::Redraw } else { Handled::NoChange }
        }).map_err(|e| e.to_string())?;
        if close { return Ok(()); }
        if redraw { state.draw(&mut win)?; }
    }
}
