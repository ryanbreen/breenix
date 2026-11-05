//! Repeat and multi-press policy, adapted from #2476 dfc2e318.
use libbreenix::input::{kind, syn, Event};
use std::collections::BTreeMap;

pub(super) fn within(first: u64, second: u64) -> bool {
    second.checked_sub(first).is_some_and(|d| d <= 400_000_000)
}
struct Held { source: Event, target: u64, next: u64, derived: u32 }
pub(super) struct Repeat { pub source: Event, pub target: u64, pub time: u64, pub derived: u32 }
pub(super) struct Timing { held: BTreeMap<(u32, u16), Held> }
impl Timing {
    pub(super) fn new() -> Self { Self { held: BTreeMap::new() } }
    /// Returns false for device repeats, which the caller must dispose as handled.
    pub(super) fn key(&mut self, event: Event, focus: Option<u64>, consumed: bool, now: u64) -> bool {
        if event.kind == kind::SYN && event.code == syn::DROPPED {
            // Reader overflow may lose releases from several devices while
            // naming only one device in its marker. Discard held-key state.
            self.held.clear();
            return true;
        }
        let slot = (event.device, event.code);
        match event.value {
            0 => { self.held.remove(&slot); }
            1 if !consumed && !matches!(event.code, 1 | 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126) => {
                if let Some(target) = focus { self.held.insert(slot, Held { source: event, target, next: now.saturating_add(500_000_000), derived: 2 }); }
            }
            2 => return false,
            _ => {}
        }
        true
    }
    pub(super) fn deadline(&self) -> Option<u64> { self.held.values().map(|h| h.next).min() }
    pub(super) fn focus(&mut self, focus: Option<u64>) { self.held.retain(|_, h| Some(h.target) == focus); }
    pub(super) fn due(&mut self, now: u64, focus: Option<u64>) -> Vec<Repeat> {
        self.focus(focus);
        let mut out = Vec::new();
        for h in self.held.values_mut() {
            while h.next <= now && h.derived < (1 << 22) {
                out.push(Repeat { source: h.source, target: h.target, time: h.next, derived: h.derived });
                h.derived += 1;
                h.next = h.next.saturating_add(50_000_000);
            }
        }
        out
    }
}
#[derive(Clone, Copy)]
struct Click { target: u64, point: (i32, i32), time: u64, count: u32 }
pub(super) struct Clicks { last: BTreeMap<(u32, u16), Click>, active: BTreeMap<(u32, u16), u32> }
impl Clicks {
    pub(super) fn new() -> Self { Self { last: BTreeMap::new(), active: BTreeMap::new() } }
    pub(super) fn press(&mut self, device: u32, button: u16, target: u64, point: (i32, i32), time: u64) -> u32 {
        let slot = (device, button);
        let count = if self.last.get(&slot).is_some_and(|c| c.count == 1 && c.target == target && within(c.time, time) && (i64::from(c.point.0) - i64::from(point.0)).abs() <= 4 && (i64::from(c.point.1) - i64::from(point.1)).abs() <= 4) { 2 } else { 1 };
        self.last.insert(slot, Click { target, point, time, count });
        self.active.insert(slot, count);
        count
    }
    pub(super) fn release(&mut self, device: u32, button: u16) -> u32 { self.active.remove(&(device, button)).unwrap_or(1) }
}
