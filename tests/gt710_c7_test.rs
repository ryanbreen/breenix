#[path = "../kernel/src/gpu/gk208/production_policy.rs"]
mod policy;
use policy::{Stage, Startup, Guard, start, within_one_percent};

#[derive(Default)]
struct Device { fail: Option<Stage>, events: Vec<Stage>, restores: Vec<&'static str>, restore_fails: bool }
impl Startup for Device {
    fn step(&mut self, stage: Stage) -> Result<(), &'static str> {
        self.events.push(stage);
        if self.fail == Some(stage) { Err("injected hardware failure") } else { Ok(()) }
    }
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
