//! W19: input response coalescing before the synchronous composite.
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision { Compose, WaitUntil(u64) }
// Reserve dispatch time beyond the observed complete composition cost.
const GUARD_NS: u64 = 1_500_000;
/// W20: bound the response hold by the first reachable observed presentation edge.
pub fn decide(now: u64, pending: bool, edge: u64, period: u64, work: u64) -> Decision {
    if !pending || period == 0 { return Decision::Compose; }
    let reachable = now.saturating_add(work).saturating_add(GUARD_NS);
    let next = edge.saturating_add((reachable.saturating_sub(edge) / period + 1).saturating_mul(period));
    let latest = next.saturating_sub(work.saturating_add(GUARD_NS));
    if now < latest { Decision::WaitUntil(latest) } else { Decision::Compose }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing { pub presented_ns: u64, pub period_ns: u64, pub prepare_ns: u64, pub lead_ns: u64 }
impl Timing {
    pub fn parse(serial: u64, text: &str) -> Result<Self, &'static str> {
        let field=|prefix: &str| text.split_whitespace().find_map(|word| word.strip_prefix(prefix))
            .and_then(|value| value.parse::<u64>().ok()).ok_or("compositor timing field absent");
        if serial==0 || field("pacing_serial=")?!=serial { return Err("compositor timing serial differs"); }
        let timing=Self { presented_ns:field("presentation_ns=")?, period_ns:field("period_ns=")?,
            prepare_ns:field("prepare_ns=")?, lead_ns:field("accept_lead_ns=")? };
        if timing.presented_ns==0 || timing.lead_ns>timing.period_ns { return Err("invalid compositor timing"); }
        Ok(timing)
    }
}
pub type Serial = (u64, u32);
#[derive(Default)]
pub struct Pacer {
    routed: BTreeMap<u64, Serial>,
    edge: Option<u64>,
    period: Option<u64>,
    work: Option<u64>,
    waiting: Option<u64>,
    recent: [u64; 8],
    next: usize,
}
impl Pacer {
    pub fn delivered(&mut self, window: u64, serial: Serial) {
        if serial == (0, 0) { return; }
        self.routed.entry(window).and_modify(|old| {
            if serial > *old { *old = serial; }
        }).or_insert(serial);
    }
    pub fn refresh(&mut self, mut committed: impl FnMut(u64, Serial) -> Option<Serial>) {
        self.routed.retain(|id, wanted| {
            committed(*id, *wanted).is_none_or(|done| done < *wanted)
        });
    }
    pub fn decision(&mut self, now: u64) -> Decision {
        if self.routed.is_empty() { self.waiting = None; return Decision::Compose; }
        if let Some(deadline) = self.waiting {
            return if now < deadline { Decision::WaitUntil(deadline) } else { Decision::Compose };
        }
        let decision = match (self.edge,self.period,self.work) {
            (Some(edge),Some(period),Some(work)) => decide(now,true,edge,period,work),
            _ => Decision::Compose,
        };
        if let Decision::WaitUntil(deadline) = decision { self.waiting = Some(deadline); }
        decision
    }
    pub fn presented(&mut self, timing: Timing) {
        if self.period!=Some(timing.period_ns) { self.recent=[0;8];self.next=0; }
        self.recent[self.next%self.recent.len()]=timing.prepare_ns;
        self.next=self.next.wrapping_add(1);
        self.work=Some(self.recent.iter().copied().max().unwrap_or(0).saturating_add(timing.lead_ns));
        self.period=Some(timing.period_ns);
        self.edge=Some(timing.presented_ns);
        self.routed.clear();
        self.waiting=None;
    }
}
