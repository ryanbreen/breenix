extern crate alloc;
#[path = "../kernel/src/gpu/gk208/present.rs"]
mod present;
use present::{Io, Surface};
use std::collections::BTreeMap;

struct Display {
    registers: BTreeMap<usize, u32>,
    confirmations: usize,
    submissions: usize,
}
impl Io for Display {
    fn clear_notifier(&mut self) -> Result<(), &'static str> { Err("unexpected UPDATE") }
    fn submit(&mut self, _: &[(u32, u32)]) -> Result<(), &'static str> {
        self.submissions += 1; Err("unexpected UPDATE")
    }
    fn complete(&mut self) -> Result<u32, &'static str> { Err("unexpected UPDATE") }
    fn read(&mut self, r: usize) -> u32 { *self.registers.get(&r).unwrap_or(&0) }
}
fn surface() -> Surface {
    Surface { resource: 44, address: 0x200000, width: 1280, height: 800,
        pitch: 5120, block_height: 0, format: 0, dma: 1 }
}
fn display() -> Display {
    let mut d = Display { registers: BTreeMap::new(), confirmations: 0, submissions: 0 };
    for &(m, v) in &surface().methods(0).unwrap()[..5] {
        for bank in [0x640000, 0x660000] { d.registers.insert(bank + m as usize, v); }
    }
    d
}
#[test]
fn unchanged_binding_requires_a_new_evo_acknowledgement() {
    let mut d = display();
    let r = present::retained(&mut d, 0, surface(), |io| {
        io.confirmations += 1; Ok(0xa001)
    }).unwrap().unwrap();
    assert_eq!(d.confirmations, 1); assert_eq!(d.submissions, 0);
    assert_eq!(r.notifier, 0xa001); assert_eq!(r.active, r.armed);
    assert_eq!(u64::from(r.active[0]) << 8, surface().address);
}
#[test]
fn every_mismatched_surface_field_requires_the_update_path() {
    for bank in [0x640000, 0x660000] {
        for &(m, _) in &surface().methods(0).unwrap()[..5] {
            let mut d = display();
            *d.registers.get_mut(&(bank + m as usize)).unwrap() ^= 1;
            assert!(present::retained(&mut d, 0, surface(), |io| {
                io.confirmations += 1; Ok(1)
            }).unwrap().is_none());
            assert_eq!(d.confirmations, 0);
        }
    }
}
#[test]
fn old_latched_state_cannot_replace_a_fresh_notifier() {
    assert!(present::retained(&mut display(), 0, surface(), |_| Ok(0)).is_err());
    assert!(present::retained(&mut display(), 0, surface(), |_| Err("EVO timeout")).is_err());
}
#[test]
fn both_banks_are_rechecked_after_evo_completion() {
    for bank in [0x640000, 0x660000] {
        assert!(present::retained(&mut display(), 0, surface(), |io| {
            io.registers.insert(bank + 0x460, 0x300000 >> 8); Ok(1)
        }).is_err());
    }
}
#[test]
fn changed_surface_still_requires_update_and_its_notifier() {
    assert!(present::flush(&mut display(), 0, surface()).is_err());
    assert!(surface().dma_words(1 << 30).is_ok());
}
