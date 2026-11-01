#[path = "../kernel/src/gpu/gk208/production_policy.rs"]
mod policy;
use policy::{Stage, Startup, Guard, start, within_one_percent};

#[derive(Default)]
struct Device { fail: Option<Stage>, events: Vec<Stage>, restores: Vec<&'static str>, restore_fails: bool, memory:bool, gpc:bool, fail_before:bool }
impl Startup for Device {
    fn step(&mut self, stage: Stage) -> Result<(), &'static str> {
        self.events.push(stage);
        if !self.fail_before {
            self.memory |= stage==Stage::MemoryIntermediate;
            self.gpc |= stage==Stage::Gpc;
        }
        if self.fail == Some(stage) { Err("injected hardware failure") } else { Ok(()) }
    }
    fn changed(&self)->(bool,bool) {(self.gpc,self.memory)}
    fn restore_gpc(&mut self) -> Result<(), &'static str> {
        self.restores.push("gpc");
        if self.restore_fails { Err("restore timeout") } else { Ok(()) }
    }
    fn restore_memory(&mut self) -> Result<(), &'static str> {
        self.restores.push("memory"); Ok(())
    }
}

#[test]
fn every_failed_stage_falls_back_and_restores_continue_after_an_error() {
    for stage in Stage::ALL {
        let mut d = Device { fail: Some(stage), restore_fails: true, ..Device::default() };
        let outcome = start(&mut d, false);
        assert!(!outcome.full);
        assert_eq!(outcome.failure, Some("injected hardware failure"));
        assert_eq!(outcome.failed_stage, Some(stage));
        assert_eq!(d.events.last(), Some(&stage));
        let changed_memory = d.events.contains(&Stage::MemoryIntermediate);
        let changed_gpc = d.events.contains(&Stage::Gpc);
        assert_eq!(d.restores.contains(&"gpc"), changed_gpc);
        assert_eq!(d.restores.contains(&"memory"), changed_memory);
        if changed_gpc { assert_eq!(d.restores, ["gpc", "memory"]); }
    }
    let mut d = Device::default();
    assert!(start(&mut d, false).full);
    assert_eq!(d.events, Stage::ALL);
    assert!(d.restores.is_empty());
    let mut d = Device::default();
    assert!(!start(&mut d, true).full);
    assert!(d.events.is_empty());
}

#[test]
fn readback_boundary_accepts_one_percent_and_refuses_missing_or_more() {
    for target in [405000, 900000, 1307000, 81688] {
        for delta in [0, target / 100] {
            assert!(within_one_percent(Ok(target + delta), target));
            assert!(within_one_percent(Ok(target - delta), target));
        }
        assert!(!within_one_percent(Ok(target + target / 100 + 1), target));
        assert!(!within_one_percent(Ok(target - target / 100 - 1), target));
        assert!(!within_one_percent(Err("read failed"), target));
    }
}

#[test]
fn guard_trips_at_85_or_sensor_loss_and_never_raises_again() {
    for hot in [Ok(85), Ok(86), Ok(120), Err("sensor lost")] {
        let mut g = Guard::new();
        for cool in [0, 34, 84] { assert!(!g.sample(Ok(cool))); }
        assert!(g.sample(hot));
        for t in [Ok(34), Ok(85), hot] { assert!(!g.sample(t)); }
        assert!(g.tripped());
    }
}

#[test]
fn failure_before_the_first_write_skips_memory_restoration() {
 let mut d=Device{fail:Some(Stage::MemoryIntermediate),fail_before:true,..Device::default()};
 let out=start(&mut d,false);assert!(!out.full);assert!(d.restores.is_empty());
}
#[test]
fn failed_guard_restores_retry_each_second_and_critical_stop_latches_once() {
 let mut g=Guard::new();assert!(g.sample(Ok(85)));
 assert!(g.retry_due(0,true));assert!(!g.retry_due(250_000_000,true));
 assert!(!g.retry_due(999_999_999,true));assert!(g.retry_due(1_000_000_000,true));
 assert!(!g.critical(Ok(94),true));assert!(!g.critical(Ok(95),false));
 assert!(g.critical(Ok(95),true));assert!(!g.critical(Ok(96),true));
 assert!(!g.retry_due(2_000_000_000,false));
}

#[test]
fn clock_health_reads_gr_only_when_enabled_and_names_live_faults() {
 let mut reads=Vec::new();
 let mut gated=|r| {reads.push(r);match r {0x200=>0,0x400100=>panic!("read gated PGRAPH"),_=>0}};
 assert!(policy::clock_health(&mut gated).is_ok());
 assert!(!reads.contains(&0x400100));
 for (register,value,check) in [(0x259c,4,"fifo-fault"),(0x400100,8,"pgraph-fault"),(0x10a008,1,"pmu-pending")] {
  let failure=policy::clock_health(&mut |r|match r {0x200=>0x1000,0x10a01c=>1,_ if r==register=>value,_=>0}).unwrap_err();
  assert_eq!((failure.check,failure.register,failure.value),(check,register,value));
 }
}

#[test]
fn slow_failed_restores_back_off_from_completion_and_stop_after_three() {
 let mut g=Guard::new();assert!(g.sample(Ok(85)));
 for attempt in 0..3 {
  let start=attempt*13_000_000_000;
  assert!(g.retry_due(start,true));
  let finished=start+12_000_000_000;
  g.restore_finished(finished);
  assert!(!g.retry_due(finished,true));
  assert!(!g.retry_due(finished+999_999_999,true));
 }
 assert!(!g.retry_due(100_000_000_000,true));
}
