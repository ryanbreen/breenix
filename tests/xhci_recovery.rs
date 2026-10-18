//! Execute the driver's EP0 orchestration with controlled controller completions.
extern crate alloc;
#[path = "../kernel/src/drivers/xhci/spec.rs"]
pub mod spec;
#[path = "../kernel/src/drivers/xhci/recovery.rs"]
mod recovery;
#[path = "../kernel/src/drivers/xhci/fault.rs"]
mod fault;
use std::collections::VecDeque;
use spec::Trb;

struct Controller {
    events: VecDeque<Trb>,
    submissions: usize,
    recovered: usize,
    retries: usize,
    waits: Vec<Vec<u64>>,
}
fn event(pointer: u64, code: u8) -> Trb {
    Trb::new(32, pointer, (code as u32) << 24, (1 << 24) | (1 << 16))
}
impl Controller {
    fn new(events: Vec<Trb>) -> Self {
        Self { events: events.into(), submissions: 0, recovered: 0, retries: 0, waits: vec![] }
    }
}
impl recovery::Control for Controller {
    fn slot(&self) -> u8 { 1 }
    fn submit(&mut self) -> Vec<u64> {
        self.submissions += 1;
        let base = self.submissions as u64 * 0x100;
        vec![base, base + 16, base + 32]
    }
    fn wait(&mut self, pointers: &[u64]) -> Result<Trb, &'static str> {
        self.waits.push(pointers.to_vec());
        // The hardware adapter selects events the same way, preserving other
        // endpoints' input while synchronous EP0 waits run.
        let i = self.events.iter().position(|e| pointers.contains(&(e.parameter & !15)))
            .ok_or("MSI completion timed out")?;
        Ok(self.events.remove(i).unwrap())
    }
    fn recover(&mut self) -> Result<(), &'static str> { self.recovered += 1; Ok(()) }
    fn retry(&mut self, _: Trb, attempt: u32, after_ns: u64) {
        self.retries += 1;
        assert_eq!(attempt as usize, self.retries);
        assert_eq!(after_ns, spec::EP0_RETRY_NS);
    }
}
#[test]
fn short_read_retires_status_before_resend_and_preserves_input() {
    let input = Trb::new(32, 0x900, 1 << 24, (1 << 24) | (3 << 16));
    let mut c = Controller::new(vec![input, event(0x110, 13), event(0x120, 1), event(0x220, 1)]);
    assert_eq!(recovery::transfer(&mut c).unwrap().code(), 1);
    assert_eq!((c.submissions, c.recovered, c.retries), (2, 0, 1));
    assert_eq!(c.waits, [vec![0x100, 0x110, 0x120], vec![0x120], vec![0x200, 0x210, 0x220]]);
    assert_eq!(c.events, [input]);
}
#[test]
fn final_short_read_also_retires_status_and_keeps_attempt_limit() {
    let mut c = Controller::new((1..=3).flat_map(|n| [event(n * 0x100 + 16, 13), event(n * 0x100 + 32, 1)]).collect());
    assert_eq!(recovery::transfer(&mut c).unwrap().code(), 13);
    assert_eq!((c.submissions, c.recovered, c.retries), (3, 0, 2));
    assert!(c.events.is_empty());
    assert_eq!(c.waits.last().unwrap(), &[0x320]);
}
#[test]
fn repeated_stall_recovers_even_on_last_attempt() {
    let mut c = Controller::new(vec![event(0x100, 6), event(0x200, 6), event(0x300, 6)]);
    assert_eq!(recovery::transfer(&mut c).unwrap().code(), 6);
    assert_eq!((c.submissions, c.recovered, c.retries), (3, 3, 2));
}
#[test]
fn failed_or_mismatched_status_cannot_be_hidden_by_a_resend() {
    for code in [3, 5, 6] {
        let mut c = Controller::new(vec![event(0x110, 13), event(0x120, code)]);
        assert_eq!(recovery::transfer(&mut c).unwrap().code(), code);
        assert_eq!((c.submissions, c.retries), (1, 0));
        assert_eq!(c.recovered, usize::from(spec::halts_endpoint(code)));
    }
    for flags in [(2 << 24) | (1 << 16), (1 << 24) | (3 << 16)] {
        let mut c = Controller::new(vec![event(0x110, 13), Trb::new(32, 0x120, 1 << 24, flags)]);
        assert_eq!(recovery::transfer(&mut c).unwrap_err(), "EP0 transfer identity mismatch");
        assert_eq!(c.submissions, 1);
    }
}
#[test]
fn missing_status_does_not_send_another_setup() {
    let mut c = Controller::new(vec![event(0x110, 13)]);
    assert_eq!(recovery::transfer(&mut c).unwrap_err(), "MSI completion timed out");
    assert_eq!((c.submissions, c.retries), (1, 0));
}
