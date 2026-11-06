//! W19 production decision in a deterministic synchronous presentation pipeline.
#[path = "../userspace/programs/src/bwm/pace.rs"]
mod pace;
#[path = "../kernel/src/gpu/gk208/vblank_clock.rs"]
mod vblank_clock;
use pace::{decide, Decision};
extern crate alloc;
mod irq_safe_mutex {
    pub struct IrqSafeMutex<T>(std::sync::Mutex<T>);
    impl<T> IrqSafeMutex<T> {
        pub const fn new(value: T)->Self {Self(std::sync::Mutex::new(value))}
        pub fn lock(&self)->std::sync::MutexGuard<'_,T> {self.0.lock().unwrap()}
    }
}
#[path = "../kernel/src/window/pacing.rs"]
mod producer;

const MS: u64 = 1_000_000;
fn timing(edge: u64, prepare: u64) -> pace::Timing {
    pace::Timing { presented_ns: edge, period_ns: period(), prepare_ns: prepare, lead_ns: 0 }
}
fn period() -> u64 { vblank_clock::VblankClock::new(0, 0, 1688 * 1066, 108_000_000).at(1) }
// W18 records: release-to-accept about 4 ms; UPDATE DONE leads the edge by 114 lines.
fn presented(start: u64) -> u64 {
    let p = period();
    let eligible = start + 4 * MS + 1688 * 114 * 1_000_000_000 / 108_000_000;
    eligible.div_ceil(p) * p
}
fn finish(flight: &mut bool, now: u64) -> u64 {
    assert!(*flight);
    *flight = false;
    presented(now)
}
fn samples(typing: bool, animation: bool) -> Vec<u64> {
    let p = period();
    let mut samples = Vec::new();
    for phase in 0..101 {
        let inject = 4 * p + p * phase / 101;
        let read = if animation { inject.div_ceil(p) * p } else { inject };
        let response = read + if typing { 1500_000 } else { 500_000 } + if animation { 2 * MS } else { 0 };
        let initial = read + if typing && !animation { 500_000 } else { 0 };
        let mut now = initial;
        let mut in_flight = false;
        let mut composites = 0;
        loop {
            let pending = now < response;
            let edge = now / p * p;
            match decide(now, pending, edge, p, 4 * MS + 1688 * 114 * 1_000_000_000 / 108_000_000) {
                Decision::WaitUntil(deadline) => {
                    assert!(deadline > now);
                    // Scene readiness wakes earlier than the deadline when the client commits.
                    now = response.min(deadline);
                }
                Decision::Compose => {
                    assert!(!in_flight, "one synchronous composite in flight");
                    in_flight = true;
                    let includes_response = now >= response;
                    let done = finish(&mut in_flight, now);
                    assert!(done > now);
                    now = done;
                    composites += 1;
                    if includes_response { samples.push(now - inject); break; }
                    assert!(composites <= 2);
                }
            }
        }
    }
    samples.sort_unstable();
    samples
}
fn check(typing: bool, animation: bool, median_bound: u64) {
    let s = samples(typing, animation);
    println!("typing={typing} animation={animation} median_ns={} max_ns={}",s[50],s[100]);
    assert!(s[50] <= median_bound, "response held an extra display frame");
    assert!(s[100] <= 2*period());
}
#[test] fn pointer_response() { check(false,false,17*MS); }
#[test] fn typing_response() { check(true,false,17*MS); }
#[test] fn animated_response() { check(true,true,27*MS); }
#[test] fn response_wait_is_bounded() {
    assert_eq!(decide(0,true,0,period(),6*MS), Decision::WaitUntil(period()-6*MS-1500_000));
}
#[test]
fn no_input_and_unresponsive_input_keep_the_next_frame() {
    let p = period();
    let mut last = 0;
    for _ in 0..300 {
        assert_eq!(decide(last,false,last,p,6*MS),Decision::Compose);
        let next = presented(last);
        assert_eq!(next-last,p);
        last = next;
    }
    // W20: the nearest edge is unreachable; bound by the first reachable edge.
    assert_eq!(decide(p-2*MS,true,0,p,6*MS),Decision::WaitUntil(2*p-6*MS-1500_000));
}

#[test]
fn delivery_watermarks_and_late_wakes_do_not_extend_a_frame() {
    let p=period(); let mut pace=pace::Pacer::default();
    pace.presented(timing(p,6*MS)); pace.presented(timing(2*p,6*MS));
    let derived=(10,1);
    pace.delivered(1,derived); pace.delivered(1,(11,0));
    pace.refresh(|_,_|Some(derived));
    let Decision::WaitUntil(deadline)=pace.decision(2*p+MS) else {panic!("unhandled release must wait")};
    pace.delivered(1,(12,0));
    assert_eq!(pace.decision(2*p+2*MS),Decision::WaitUntil(deadline));
    assert_eq!(pace.decision(4*p),Decision::Compose,"late dispatch cannot roll the hold to another edge");
    pace.refresh(|_,_|Some((12,0)));
    assert_eq!(pace.decision(2*p+2*MS),Decision::Compose,"the handling commit releases the hold");
}

#[test]
fn a_cheap_complete_frame_does_not_underreserve_a_later_composition() {
    let p=period();let mut pacing=pace::Pacer::default();
    let lead=1688*114*1_000_000_000/108_000_000;
    pacing.presented(pace::Timing {lead_ns:lead,..timing(p,3_900_000)});
    pacing.presented(pace::Timing {lead_ns:lead,..timing(2*p,4*MS)});
    pacing.delivered(1,(11,0));
    let Decision::WaitUntil(deadline)=pacing.decision(2*p+MS) else {panic!("response hold missing")};
    let accepted=deadline+4*MS;
    let phase=(accepted%p)*1066/p;
    println!("learned-completion-budget accept_phase={phase} deadline_ns={deadline}");
    assert!(accepted+1688*114*1_000_000_000/108_000_000<=3*p,
        "minimum complete-frame cost sent the response past the physical UPDATE deadline");
}

#[test]
fn sparse_event_driven_composites_do_not_become_the_display_period() {
    let p=period();let mut pacing=pace::Pacer::default();
    pacing.presented(timing(3*p,6*MS));
    pacing.presented(timing(6*p,6*MS));
    pacing.delivered(1,(11,0));
    let Decision::WaitUntil(deadline)=pacing.decision(6*p+MS) else {panic!("response hold missing")};
    assert!(deadline+6*MS+1500_000<=7*p,
        "three-period application gaps became a three-period response hold");
}

#[test]
fn timing_is_bound_to_the_completed_composite_and_supports_virtual_completion() {
    producer::publish(7,100,16661186,4000000,1800000);
    let text=producer::text();
    let timing=pace::Timing::parse(7,&text).unwrap();
    assert_eq!((timing.presented_ns,timing.period_ns,timing.prepare_ns,timing.lead_ns),(100,16661186,4000000,1800000));
    assert!(pace::Timing::parse(8,&text).is_err());
    assert!(pace::Timing::parse(7,"pacing_serial=7 presentation_ns=100 period_ns=0 prepare_ns=100 accept_lead_ns=1").is_err());
    let virtual_timing=pace::Timing::parse(7,"pacing_serial=7 presentation_ns=100 period_ns=0 prepare_ns=100 accept_lead_ns=0").unwrap();
    let mut pacing=pace::Pacer::default();pacing.presented(virtual_timing);pacing.delivered(1,(10,0));
    assert_eq!(pacing.decision(101),Decision::Compose);
}
