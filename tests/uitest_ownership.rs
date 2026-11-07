#[path = "../libs/libuitest/src/ownership.rs"]
mod ownership;
use ownership::{Custody, Event, Owner, Ownership};

#[derive(Default)]
struct Display { calls: Vec<&'static str>, fail: Option<&'static str> }
impl Display {
    fn call(&mut self, name: &'static str) -> Result<(), String> {
        self.calls.push(name);
        if self.fail == Some(name) { Err(name.into()) } else { Ok(()) }
    }
}
impl Custody for Display {
    fn take_display(&mut self) -> Result<(), String> { self.call("take") }
    fn claim_compositor(&mut self) -> Result<(), String> { self.call("claim") }
    fn release_compositor(&mut self) -> Result<(), String> { self.call("release") }
    fn give_display(&mut self) -> Result<(), String> { self.call("give") }
}
fn reach(events: &[Event]) -> (Ownership, Display) {
    let mut owner = Ownership::default(); let mut display = Display::default();
    for &event in events { owner.apply(event, &mut display).unwrap(); }
    (owner, display)
}
#[test]
fn death_is_recovered_before_the_case_hands_back_to_the_runner() {
    use Event::*; use Owner::*;
    let (mut owner, mut display) = reach(&[Begin, Started]);
    assert_eq!(owner.owner(), CompositorAlive);
    owner.apply(Died, &mut display).unwrap();
    assert_eq!(owner.owner(), CompositorDied);
    owner.apply(Recover, &mut display).unwrap();
    assert_eq!(owner.owner(), SupervisorHolds);
    owner.apply(End, &mut display).unwrap();
    assert_eq!(owner.owner(), CaseEnd);
    owner.apply(Reclaim, &mut display).unwrap();
    assert_eq!(owner.owner(), RunnerHolds);
    assert_eq!(display.calls, ["take", "take", "claim", "release", "give", "take"]);
}
#[test]
fn every_illegal_transition_is_refused_without_touching_the_display() {
    use Event::*; use Owner::*;
    for (path, state, legal) in [
        (vec![], Initial, vec![Begin]),
        (vec![Begin], SupervisorReady, vec![Started, End]),
        (vec![Begin, Started], CompositorAlive, vec![Died]),
        (vec![Begin, Started, Died], CompositorDied, vec![Recover, End]),
        (vec![Begin, Started, Died, Recover], SupervisorHolds, vec![End]),
        (vec![Begin, End], CaseEnd, vec![Reclaim]),
        (vec![Begin, End, Reclaim], RunnerHolds, vec![]),
    ] {
        for event in [Begin, Started, Died, Recover, End, Reclaim] {
            let (mut owner, mut display) = reach(&path);
            assert_eq!(owner.owner(), state);
            let calls = display.calls.clone();
            let result = owner.apply(event, &mut display);
            assert_eq!(result.is_ok(), legal.contains(&event), "{state:?} -> {event:?}");
            if result.is_err() { assert_eq!(owner.owner(), state); assert_eq!(display.calls, calls); }
        }
    }
}
#[test]
fn failed_claim_or_release_never_publishes_a_completed_handoff() {
    use Event::*; use Owner::*;
    let (mut owner, mut display) = reach(&[Begin, Started, Died]);
    display.fail = Some("claim");
    assert!(owner.apply(Recover, &mut display).is_err());
    assert_eq!(owner.owner(), CompositorDied);
    display.fail = None;
    owner.apply(Recover, &mut display).unwrap();
    display.fail = Some("release");
    assert!(owner.apply(End, &mut display).is_err());
    assert_eq!(owner.owner(), SupervisorHolds);
    assert_eq!(display.calls.last(), Some(&"release"));
    display.fail = None;
    owner.apply(End, &mut display).unwrap();
    display.fail = Some("take");
    assert!(owner.apply(Reclaim, &mut display).is_err());
    assert_eq!(owner.owner(), CaseEnd);
}
