//! Execute the production transaction scope and one-shot point implementation.
extern crate self as breengel_abi;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
#[path = "../kernel/src/test_control/point.rs"] mod point;
#[path = "../kernel/src/test_control/input_replay.rs"] mod input_replay;
mod tc_point { pub const INPUT_REPORT_SKIPPED:u32=33; }
static POINT_TABLE:[point::Point;34]=[const {point::Point::new()};34];
static ENABLED:AtomicBool=AtomicBool::new(false);
static MASKED:AtomicU32=AtomicU32::new(0);
static MASKS:AtomicU32=AtomicU32::new(0);
fn enabled()->bool { ENABLED.load(Ordering::Relaxed) }
fn take(point:u32,target:u64)->Option<u64> {
    assert!(MASKED.load(Ordering::Relaxed)>0,"input point consumed without masking IRQs");
    POINT_TABLE[point as usize].reserve(target,|_|true).map(|r|r.fire())
}
fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T {
    MASKS.fetch_add(1,Ordering::Relaxed); MASKED.fetch_add(1,Ordering::Relaxed);
    let result=f(); MASKED.fetch_sub(1,Ordering::Relaxed); result
}
mod syscall { pub mod errno { pub const ENOSYS:i32=38; pub const ESRCH:i32=3; pub const EBUSY:i32=16; } }
mod task { pub mod scheduler {
    pub static TID:crate::AtomicU64=crate::AtomicU64::new(1);
    pub fn current_thread_id()->Option<u64> { Some(TID.load(crate::Ordering::Relaxed)) }
} }
#[test]
fn suppression_is_one_firing_for_the_replay_pair_and_cannot_leak_or_be_stolen() {
    use input_replay::{InputReplay,input_report_allowed};
    assert!(InputReplay::begin(7,true).is_err());
    assert!(input_report_allowed(7));
    ENABLED.store(true,Ordering::Relaxed);
    let scope=InputReplay::begin(7,true).unwrap();
    assert!(InputReplay::begin(8,false).is_err());
    assert!(input_report_allowed(8));
    task::scheduler::TID.store(2,Ordering::Relaxed);
    assert!(input_report_allowed(7));
    task::scheduler::TID.store(1,Ordering::Relaxed);
    assert!(!input_report_allowed(7)); assert!(!input_report_allowed(7));
    assert_eq!(scope.calls(),0);
    assert_eq!(POINT_TABLE[33].fired(),1);
    drop(scope);
    assert!(input_report_allowed(7));
    let scope=InputReplay::begin(7,false).unwrap();
    assert!(input_report_allowed(7)); assert!(input_report_allowed(7));
    assert_eq!(scope.calls(),2);
    drop(scope);
    // Failed preparation leaves no armed suppression for the next hardware completion.
    drop(InputReplay::begin(7,true).unwrap());
    assert!(input_report_allowed(7));
    assert_eq!(POINT_TABLE[33].fired(),1);
    assert!(MASKS.load(Ordering::Relaxed)>=8);
}
