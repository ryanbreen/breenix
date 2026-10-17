//! The GPU effort's runner view: a solid tile per case, rendered by syscall 416.
//! Expectations and judged pixels never pass through this display helper.
use super::{State, Outcome};
use crate::{gpu::{self, abi::{self, stream::encode}}, graphics};
use std::string::String;
use std::vec::Vec;

pub(super) struct GpuScreen {
    context: u64,
    target: u64,
}
impl GpuScreen {
    pub fn new() -> Self { Self { context: 0, target: 0 } }

    /// Draw the strip and present it. A failure leaves the view as it was and is the
    /// caller's to report; the cases run either way (`docs/gui/gpu.md` section 1.10).
    pub fn draw(&mut self, states: &[Vec<State>]) -> Result<(), String> {
        // The runner draws only between cases, while no case owns a test-control point,
        // so a point left armed (by a case that was killed, or before the suite started)
        // can neither alter this view nor be spent by it.
        let _ = gpu::tc_disarm_all();
        if self.context != 0 {
            let (r, c) = gpu::counters(self.context);
            if r != 0 || c.state != abi::context_state::LIVE {
                let _ = gpu::resource_destroy(self.target);
                let _ = gpu::context_destroy(self.context);
                self.context = 0;
                self.target = 0;
            }
        }
        if self.context == 0 {
            self.context = gpu::context_create().map_err(|e| std::format!("context: errno {}", -e))?;
            match gpu::scanout_target_create(abi::format::B8G8R8A8, 2560, 1600) {
                Ok(t) => self.target = t,
                Err(e) => {
                    let _ = gpu::context_destroy(self.context);
                    self.context = 0;
                    return Err(std::format!("target: errno {}", -e));
                }
            }
        }
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
        if r != 0 || p.answer != 0x1100 {
            return Err(std::format!("present: errno {}, answer {:#x}", -r, p.answer));
        }
        Ok(())
    }
}
