//! Native slots remain owned until an ordered release, even after caller exit.
#[path = "../kernel/src/gpu/gk208/queue.rs"] mod queue;
use queue::Queue;
#[test]
fn backpressure_does_not_reuse_command_or_release_storage() {
    let mut q=Queue::new();
    for s in 1..=4 {assert_eq!(q.next().unwrap(),s);q.publish(s).unwrap();}
    assert!(q.next().is_err());
    assert!(q.complete(5).is_err());
    assert_eq!(q.outstanding(),4);
    q.complete(2).unwrap();
    assert_eq!(q.next().unwrap(),5);
    assert_eq!(Queue::slot(5),Queue::slot(1));
    q.publish(5).unwrap();
    assert_eq!(q.outstanding(),3);
}
#[test]
fn descriptor_wrap_keeps_monotonic_native_identity_and_bounded_slots() {
    let mut q=Queue::new();
    for s in 1..=1030 {
        assert_eq!(q.next().unwrap(),s);
        assert!(Queue::slot(s)<4);
        assert_eq!(Queue::entry(s),(s-1)%512);
        q.publish(s).unwrap();q.complete(s).unwrap();
        assert_eq!(q.outstanding(),0);
    }
    assert!(q.complete(1).is_err());
    assert!(q.publish(1030).is_err());
    assert!(q.publish(1032).is_err());
}
