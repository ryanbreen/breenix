#![allow(dead_code)]
const BOUND_US: u64 = 2_000_000;
mod gk208 { pub mod queue { pub struct Queue; impl Queue { // QUEUE
} } }
#[derive(Clone, Copy)]
struct Identity { id: u32, instance: u64 }
struct Report { identity: Identity }
struct Session { pbdma: usize, userd: u64, report: Report }
struct Registers(StreamSample);
impl Registers {
    fn read(&mut self, address: usize) -> u32 {
        match address { 0x40120 => self.0.channel, 0x40014 => self.0.get,
            0x40028 => self.0.reference, _ => self.0.bind }
    }
}
fn read(regs: &mut Registers, _: u64, _: u64) -> Result<u32, &'static str> { Ok(regs.0.userd_get) }
struct Monitor<'a> { session: &'a Session, regs: &'a mut Registers, total: u64, sequence: u32, queued_put: Option<u32> }
// POLL
// PREDICATE
// AWAIT
struct Clock { now: u64, snapshots: u32 }
impl Poll for Clock {
    fn snapshot(&mut self) -> Result<(u32, u64, bool), &'static str> { self.snapshots += 1; Ok((0, 0, false)) }
    fn now_us(&mut self) -> u64 { self.now }
    fn relax(&mut self) { self.now += 10; }
}
#[test]
fn resident_progress_and_binding() {
    let id = Identity { id: 3, instance: 0x123000 };
    let sample = || StreamSample { channel: 0x2003, get: 5, reference: 5, userd_get: 5, bind: 0x80000123 };
    assert!(stream_consumed(sample(), id, 5, None));
    assert!(stream_consumed(sample(), id, 5, Some(5)));
    let mut partial = sample(); partial.get = 4;
    assert!(!stream_consumed(partial, id, 5, None));
    let mut unbound = sample(); unbound.bind = 0;
    assert!(!stream_consumed(unbound, id, 5, None));
}
#[test]
fn cold_wait_refuses_at_bound() {
    let mut clock = Clock { now: 0, snapshots: 0 };
    assert_eq!(await_consumed(&mut clock), Err("GR control stream not consumed; backing retained"));
    assert_eq!(clock.now, 2_000_000);
    assert_eq!(clock.snapshots, 200_001);
}
