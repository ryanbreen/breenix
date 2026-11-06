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
pub type Serial = (u64, u32);
#[derive(Default)]
pub struct Pacer {
    routed: BTreeMap<u64, Serial>,
    edge: Option<u64>,
    period: Option<u64>,
    work: Option<u64>,
    waiting: Option<u64>,
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
    pub fn presented(&mut self, began: u64, ended: u64) {
        // COMPOSITE returns after publication on both physical and virtual devices.
        // Learn conservative deadlines from observed completions, without backend constants.
        let elapsed = ended.saturating_sub(began);
        if elapsed != 0 { self.work = Some(self.work.map_or(elapsed, |old| old.min(elapsed))); }
        if let Some(old) = self.edge {
            let span = ended.saturating_sub(old);
            if span != 0 { self.period = Some(self.period.map_or(span, |p| p.min(span))); }
        }
        self.edge = Some(ended);
        self.routed.clear();
        self.waiting = None;
    }
}
