//! Console-suite wire records; the digest covers the exact uploaded row bytes.
use alloc::{format, string::String};
#[cfg(any(test, feature = "testing", feature = "gt710_core_probe"))]
use crate::gpu_completion::{CeSample, Request};

#[cfg(any(test, feature = "testing", feature = "gt710_core_probe"))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Control { Clean, WithheldRelease, StaleEpoch, MaskedClass }
#[cfg(any(test, feature = "testing", feature = "gt710_core_probe"))]
impl Control {
    pub fn name(self) -> &'static str {
        match self { Self::Clean => "clean", Self::WithheldRelease => "withheld-release",
            Self::StaleEpoch => "stale-epoch", Self::MaskedClass => "masked-class" }
    }
}
#[cfg(any(test, feature = "testing", feature = "gt710_core_probe"))]
pub struct Completion<'a> {
    pub name: &'a str,
    pub control: &'a str,
    pub request: Request,
    pub sample: CeSample,
    pub kind: u32,
    pub answer: u32,
    pub elapsed_us: u64,
    pub outcome: &'a str,
}
#[cfg(any(test, feature = "testing", feature = "gt710_core_probe"))]
impl Completion<'_> {
    pub fn encode(&self) -> String {
        let r = self.request;
        let s = self.sample;
        format!(concat!("GT710_CORE {{\"schema\":1,\"type\":\"completion\",\"case\":\"{}\",\"control\":\"{}\",",
            "\"context\":{},\"epoch\":{},\"sequence\":{},\"caller\":{},\"resource\":{},\"generation\":{},",
            "\"kind\":{},\"answer\":{},\"observed_context\":{},\"observed_epoch\":{},\"observed_sequence\":{},",
            "\"consumed\":{},\"healthy\":{},\"elapsed_us\":{},\"outcome\":\"{}\"}}\n"),
            self.name, self.control, r.context, r.epoch, r.sequence, r.caller, r.resource, r.generation,
            self.kind, self.answer, s.context, s.epoch, s.sequence, s.consumed, s.healthy,
            self.elapsed_us, self.outcome)
    }
}
#[cfg(any(test, feature = "testing", feature = "gt710_core_probe"))]
pub fn caps(bytes: &[u8; 512]) -> String {
    let hex: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
    format!("GT710_CORE {{\"schema\":1,\"type\":\"caps\",\"bytes\":512,\"hex\":\"{hex}\"}}\n")
}
pub fn anchor(rows: &[u8]) -> String {
    let digest = crate::block::install::sha256::sha256(rows);
    let hex: String = digest.iter().map(|v| format!("{v:02x}")).collect();
    format!("GT710_CORE_BULK {} {hex}", rows.len())
}

/// Boot-global maximum through first-CAPS startup, without resetting the gauge
/// or discarding an earlier long interval. The native gate keeps its 500 us bound.
pub fn startup(irq_masked_max_us: u64, ready: bool) -> String {
    format!("GT710_CORE_STARTUP {{\"schema\":1,\"irq_masked_max_us\":{irq_masked_max_us},\"ready\":{ready}}}\n")
}

pub fn startup_anchor(row: &[u8]) -> String {
    anchor(row).replacen("GT710_CORE_BULK", "GT710_CORE_STARTUP_BULK", 1)
}
