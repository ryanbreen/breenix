//! The GPU effort's runner view: a solid tile per case, rendered by syscall 416.
//! This display helper takes case states, rather than expectations or judged pixels.
use super::{State, Outcome};
use crate::{gpu::{self, abi::{self, stream::encode}}, graphics, time};
use std::string::String;
use std::vec::Vec;

pub(super) struct GpuScreen {
    context: u64,
    target: u64,
}
impl GpuScreen {
    pub fn new() -> Self { Self { context: 0, target: 0 } }

    /// Create the view's context and scanout target before the first case, as the first
    /// `draw` would. The runner opens the view before its memory record's first sample and
    /// keeps it for the final panel, so both samples count it.
    pub fn open(&mut self) -> Result<(), String> {
        // As in `draw`: no test-control point can alter the view or be spent by it.
        let _ = gpu::tc_disarm_all();
        self.create()
    }

    /// Draw the strip and present it. A failure leaves the view as it was and is the
    /// caller's to report; the cases run either way (`docs/gui/gpu.md` section 1.10).
    pub fn draw(&mut self, states: &[Vec<State>]) -> Result<(), String> {
        // The runner draws only between cases, while no case owns a test-control point,
        // so a point left armed (by a case that was killed, or before the suite started)
        // can neither alter this view nor be spent by it.
        let _ = gpu::tc_disarm_all();
        if self.context != 0 {
            let (r, c) = gpu::counters(self.context);
            // A context still warming (state 3) accepts the same work as a warmed one.
            if r != 0 || !matches!(c.state, abi::context_state::LIVE | abi::context_state::WARMING) {
                let _ = gpu::resource_destroy(self.target);
                let _ = gpu::context_destroy(self.context);
                self.context = 0;
                self.target = 0;
            }
        }
        self.create()?;
        let mut stream = Vec::new();
        encode::begin(&mut stream, self.target);
        encode::clear(&mut stream, 0xFF18_1512);
        // One row per category; within a row one tile per case in manifest order.
        // Blue is running, grey pending, green pass and red fail. No skipped colour:
        // a GPU skip is displayed as a failure as well as reported by the runner.
        for (row, cases) in states.iter().enumerate() {
            for (column, state) in cases.iter().enumerate() {
                let colour = match state {
                    State::Pending => 0xFF48_4542,
                    State::Running => 0xFF38_94F2,
                    State::Done(Outcome::Pass { .. }) => 0xFF45_C46A,
                    State::Done(Outcome::Fail { .. } | Outcome::Skip { .. }) => 0xFFE8_4D42,
                };
                let (x, y) = (48 + column as i32 * 80, 48 + row as i32 * 160);
                encode::draw(&mut stream, abi::pipeline::SOLID, colour, &[[x, y, x + 64, y + 112]]);
            }
        }
        let (r, s) = gpu::submit(self.context, &stream);
        if r != 0 { return Err(std::format!("submit: errno {}", -r)); }
        let (r, w) = gpu::wait(self.context, s.fence, 2_000_000_000);
        if r != 0 { return Err(std::format!("wait: errno {}, status {}", -r, w.status)); }
        graphics::take_over_display().map_err(|e| std::format!("display ownership: {e:?}"))?;
        let (r, p) = gpu::present(self.context, self.target);
        // The device's native success answer (gpu.md section 1.8, ruling 47).
        let success = if gpu::caps().1.backend == abi::backend::SVGA3D_DX { 1 } else { 0x1100 };
        if r != 0 || p.answer != success {
            return Err(std::format!("present: errno {}, answer {:#x}", -r, p.answer));
        }
        Ok(())
    }

    /// Create the context and its 2560x1600 scanout target unless the view has them, and
    /// wait for the context's warm-up.
    fn create(&mut self) -> Result<(), String> {
        if self.context != 0 {
            return Ok(());
        }
        self.context = gpu::context_create().map_err(|e| std::format!("context: errno {}", -e))?;
        match gpu::scanout_target_create(abi::format::B8G8R8A8, 2560, 1600) {
            Ok(t) => self.target = t,
            Err(e) => {
                let _ = gpu::context_destroy(self.context);
                self.context = 0;
                return Err(std::format!("target: errno {}", -e));
            }
        }
        self.settle();
        Ok(())
    }

    /// Wait, bounded, for a new context's catalogue warm-up. The runner draws before it
    /// starts a case, and the device warms one context at a time, lowest handle first, so
    /// a view context still warming would hold the next case's contexts behind it inside
    /// that case's limit.
    fn settle(&self) {
        let start = super::monotonic_ns();
        loop {
            let (r, c) = gpu::counters(self.context);
            if r != 0 || c.state != abi::context_state::WARMING {
                return;
            }
            let now = super::monotonic_ns();
            if start.zip(now).is_none_or(|(start, now)| now - start >= SETTLE_LIMIT_NS) {
                return;
            }
            let _ = time::sleep_ms(5);
        }
    }
}

/// How long the view waits for its context's warm-up: several times the slowest
/// target's catalogue warm-up, and well inside the host's progress supervision.
const SETTLE_LIMIT_NS: i128 = 10_000_000_000;

/// Finish the display handoff after the cases have been reaped. An exited owner's
/// target remains held until the console's restore completes, independently of
/// process reclaim receipts and of a temporarily steady free-page count.
pub(super) fn await_console() -> Result<(), String> {
    // Without a 3D device there is no GPU console to restore. ENODEV from
    // MODES alone is not enough: a present but failed device also returns it.
    let (r, caps) = gpu::caps();
    if r != 0 {
        return Err(std::format!("display handoff: CAPS errno {}", -r));
    }
    if caps.status == abi::status::NO_DEVICE {
        return Ok(());
    }
    // The runner may still own its panel; give it back before waiting for the
    // console. If the cases already displaced it, give-back refuses the non-owner.
    let _ = graphics::give_back_display();
    let start = super::monotonic_ns();
    loop {
        let (r, modes) = gpu::modes();
        if r != 0 {
            return Err(std::format!("display handoff: MODES errno {}", -r));
        }
        if modes.owner_depth == 0 && modes.binding_known != 0
            && modes.console_resource != 0 && modes.scanout_resource == modes.console_resource
        {
            return Ok(());
        }
        if start.zip(super::monotonic_ns()).is_none_or(|(start, now)| now - start >= 5_000_000_000) {
            return Err(String::from("display handoff: console binding did not complete within 5 s"));
        }
        let _ = time::sleep_ms(5);
    }
}
