//! Console-suite wire records; the digest covers the exact uploaded row bytes.
use alloc::{format, string::String};
use crate::gpu_completion::{CeSample, Request};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Control { Clean, WithheldRelease, StaleEpoch, MaskedClass }
impl Control {
    pub fn name(self) -> &'static str {
        match self { Self::Clean => "clean", Self::WithheldRelease => "withheld-release",
            Self::StaleEpoch => "stale-epoch", Self::MaskedClass => "masked-class" }
    }
}
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
pub fn caps(bytes: &[u8; 512]) -> String {
    let hex: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
    format!("GT710_CORE {{\"schema\":1,\"type\":\"caps\",\"bytes\":512,\"hex\":\"{hex}\"}}\n")
}
pub fn anchor(rows: &[u8]) -> String {
    let digest = crate::block::install::sha256::sha256(rows);
    let hex: String = digest.iter().map(|v| format!("{v:02x}")).collect();
    format!("GT710_CORE_BULK {} {hex}", rows.len())
}
