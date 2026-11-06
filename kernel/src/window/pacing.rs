//! Operational timing for the compositor's next response deadline.
//! The status text keeps these separate from the compositor's progress ACK.
use alloc::{format, string::String};
#[derive(Clone, Copy)]
struct Timing { serial: u64, presented: u64, period: u64, prepare: u64, lead: u64 }
static TIMING: crate::irq_safe_mutex::IrqSafeMutex<Timing> = crate::irq_safe_mutex::IrqSafeMutex::new(
    Timing { serial: 0, presented: 0, period: 0, prepare: 0, lead: 0 });
pub fn publish(serial: u64, presented: u64, period: u64, prepare: u64, lead: u64) {
    *TIMING.lock() = Timing { serial, presented, period, prepare, lead };
}
pub fn text() -> String {
    let t = *TIMING.lock();
    format!("pacing_serial={} presentation_ns={} period_ns={} prepare_ns={} accept_lead_ns={}\n",
        t.serial, t.presented, t.period, t.prepare, t.lead)
}
