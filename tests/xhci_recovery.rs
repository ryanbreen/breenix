//! Execute the driver's EP0 and device-reset orchestration with controlled
//! controller completions and attach outcomes.
extern crate alloc;
#[path = "../kernel/src/drivers/xhci/spec.rs"]
pub mod spec;
#[path = "../kernel/src/drivers/xhci/recovery.rs"]
mod recovery;
#[path = "../kernel/src/drivers/xhci/fault.rs"]
mod fault;
use std::collections::VecDeque;
use recovery::Attach;
use spec::Trb;

struct Controller {
    events: VecDeque<Trb>,
    submissions: usize,
    recovered: usize,
    retries: usize,
    retried: Vec<u8>,
    waits: Vec<Vec<u64>>,
}
fn event(pointer: u64, code: u8) -> Trb {
    Trb::new(32, pointer, (code as u32) << 24, (1 << 24) | (1 << 16))
}
impl Controller {
    fn new(events: Vec<Trb>) -> Self {
        Self { events: events.into(), submissions: 0, recovered: 0, retries: 0, retried: vec![], waits: vec![] }
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
    fn retry(&mut self, event: Trb, attempt: u32, after_ns: u64) {
        self.retries += 1;
        self.retried.push(event.code());
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
fn failed_status_after_short_read_is_recovered_and_sent_again() {
    for code in [3, 4, 6] {
        let mut c = Controller::new(vec![event(0x110, 13), event(0x120, code), event(0x220, 1)]);
        assert_eq!(recovery::transfer(&mut c).unwrap().code(), 1);
        assert_eq!((c.submissions, c.recovered, c.retries), (2, 1, 1));
        assert_eq!(c.retried, [code]);
        assert_eq!(c.waits, [vec![0x100, 0x110, 0x120], vec![0x120], vec![0x200, 0x210, 0x220]]);
    }
}
#[test]
fn failed_status_is_reported_in_place_of_the_short_read() {
    // Persisting to the last attempt: recovered each time, then reported.
    let mut c = Controller::new((1..=3).flat_map(|n| [event(n * 0x100 + 16, 13), event(n * 0x100 + 32, 6)]).collect());
    assert_eq!(recovery::transfer(&mut c).unwrap().code(), 6);
    assert_eq!((c.submissions, c.recovered, c.retries), (3, 3, 2));
    assert_eq!(c.retried, [6, 6]);
    // A completion code that does not halt EP0 is reported at once.
    let mut c = Controller::new(vec![event(0x110, 13), event(0x120, 5)]);
    assert_eq!(recovery::transfer(&mut c).unwrap().code(), 5);
    assert_eq!((c.submissions, c.recovered, c.retries), (1, 0, 0));
}
#[test]
fn mismatched_status_completion_is_an_error() {
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

/// A root port whose attaches end as `outcomes` says, then `Refused`.
struct Port {
    connected: bool,
    outcomes: VecDeque<Attach>,
    resets: Vec<u32>,
}
impl Port {
    fn new(outcomes: Vec<Attach>) -> Self {
        Self { connected: true, outcomes: outcomes.into(), resets: vec![] }
    }
}
impl recovery::Port for Port {
    fn connected(&self) -> bool { self.connected }
    fn reset(&mut self, reset: u32) -> Attach {
        self.resets.push(reset);
        self.outcomes.pop_front().unwrap_or(Attach::Refused)
    }
}
#[test]
fn a_third_device_reset_is_refused() {
    let mut p = Port::new(vec![]);
    assert_eq!(recovery::reset_while_refused(&mut p, Attach::Refused), recovery::DEVICE_RESETS);
    assert_eq!(p.resets, [1, 2]);
}
#[test]
fn resets_stop_once_settled_or_disconnected() {
    let mut p = Port::new(vec![]);
    assert_eq!(recovery::reset_while_refused(&mut p, Attach::Settled), 0);
    let mut p = Port::new(vec![Attach::Settled]);
    assert_eq!(recovery::reset_while_refused(&mut p, Attach::Refused), 1);
    assert_eq!(p.resets, [1]);
    let mut p = Port::new(vec![]);
    p.connected = false;
    assert_eq!(recovery::reset_while_refused(&mut p, Attach::Refused), 0);
    assert!(p.resets.is_empty());
}
/// The reset-device exercise's two refusals use the whole budget: the
/// attach on the second reset is the one that configures the keyboard.
struct Exercise {
    fault: fault::Injection,
    resets: Vec<u32>,
}
impl recovery::Port for Exercise {
    fn connected(&self) -> bool { true }
    fn reset(&mut self, reset: u32) -> Attach {
        self.resets.push(reset);
        if self.fault.refuse_configuration(2) { Attach::Refused } else { Attach::Settled }
    }
}
#[test]
fn reset_device_exercise_configures_on_the_last_reset() {
    let mut e = Exercise { fault: fault::Injection::new(), resets: vec![] };
    e.fault.arm(fault::Mode::ResetDevice, 2);
    let first = if e.fault.refuse_configuration(2) { Attach::Refused } else { Attach::Settled };
    assert_eq!(recovery::reset_while_refused(&mut e, first), 2);
    assert_eq!((e.resets.as_slice(), e.fault.refusals), ([1, 2].as_slice(), 2));
}
