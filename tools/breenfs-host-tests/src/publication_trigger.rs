//! E24 coordinator ledger 558(x2): background reservation publication.
use super::*;
use std::{sync::mpsc, time::Instant};

fn attribution() -> u64 {
    let mut publications = 0;
    for event in measurement::take_events() {
        if let measurement::Event::Admission { attempts, pressure } = event {
            if pressure.is_empty() {
                assert_eq!(attempts.len(), 1);
                assert_eq!(attempts[0].error, None);
                assert_eq!(attempts[0].refusal, None);
            } else {
                // Keep 544/549's exact refusal/publication/success pairing.
                assert_eq!(pressure, [true]);
                assert_eq!(attempts.len(), 2);
                assert_eq!(attempts[0].error, Some(ENOSPC));
                assert!(attempts[0].refusal.is_some_and(|[_, _, _, blocks, available]|
                    blocks > available));
                assert_eq!(attempts[1].error, None);
                assert_eq!(attempts[1].refusal, None);
                publications += 1;
            }
        }
    }
    assert_eq!(measurement::take_work(), [publications, 0, publications]);
    publications
}

#[test]
fn populated_replay_publishes_reservations_in_background() {
    let start = Instant::now();
    let fs = outstanding_grants::fixture();
    let tmp = fs.lookup(2, b"tmp").unwrap();
    {
        let s = fs.inner.state.lock();
        assert!((16000..=16400).contains(&s.durable.free_blocks));
        assert_eq!(s.durable.global_reserve, 4096);
        println!("TRIGGER_GEOMETRY free={} reserve={}", s.durable.free_blocks, s.durable.global_reserve);
    }
    // A distinct actor executes the production worker pass with now_ns=0.
    // The rendezvous provides a deterministic service opportunity after each
    // operation, without a periodic deadline or scheduling/timing assertion.
    let (send, receive) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let worker_fs = fs.clone();
    let worker = std::thread::spawn(move || {
        while receive.recv().is_ok() {
            measurement::take_work();
            let worked = worker_fs.commit_due(0).unwrap();
            let counts = measurement::take_work();
            assert_eq!(counts, [0, 0, u64::from(worked)]);
            done.send(worked).unwrap();
        }
    });
    let mut foreground = 0;
    let mut background = 0;
    let mut exceeded = 0;
    let mut peak = 0;
    let mut rows = 0;
    let mut observe = |kind: &str, index: usize| {
        foreground += attribution();
        let work = super::super::btree::measurement::take_mutations();
        let (outstanding, trigger) = {
            let s = fs.inner.state.lock();
            (s.reservations.outstanding(), s.durable.free_blocks.saturating_sub(s.durable.global_reserve) / 2)
        };
        // Each replay operation has one successful worst-case grant. Return
        // amounts are deliberately not used to enlarge this upper bound.
        let grant = work[1];
        let bounded = outstanding <= trigger + grant;
        exceeded += usize::from(!bounded);
        peak = peak.max(outstanding);
        rows += 1;
        println!("TRIGGER {kind}/{index} outstanding={outstanding} trigger={trigger} grant={grant} bounded={bounded}");
        send.send(()).unwrap();
        background += u64::from(completed.recv().unwrap());
    };
    measurement::take_events();
    measurement::take_work();
    super::super::btree::measurement::take_mutations();
    let mut inodes = Vec::new();
    for i in 0..300 {
        let name = alloc::format!("f{i:03}xxxxxxxxx");
        let ino = fs.create(tmp, name.as_bytes(), NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) }).unwrap();
        observe("create", i);
        let node = fs.open(ino).unwrap();
        assert_eq!(fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false }, b"x").unwrap().bytes, 1);
        drop(node);
        observe("write", i);
        inodes.push((name, ino));
    }
    let parent = fs.getattr(tmp).unwrap().attrs;
    for (i, (name, ino)) in inodes.into_iter().enumerate() {
        fs.remove(tmp, name.as_bytes(), Remove { directory: false, expected: ino, owner: 0, parent }).unwrap();
        observe("unlink", i);
    }
    drop(observe);
    drop(send);
    worker.join().unwrap();
    println!("TRIGGER_SUMMARY rows={rows} foreground={foreground} background={background} exceeded={exceeded} peak={peak} seconds={:.3}", start.elapsed().as_secs_f64());

    // Force the designed backstop independently of worker scheduling, on the
    // same mount: retained capacity blocks a grant that fits after publishing.
    fs.fsync(&fs.open(tmp).unwrap(), false).unwrap();
    fs.create(tmp, b"backstop", NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) }).unwrap();
    measurement::take_events(); measurement::take_work();
    let state = fs.inner.state.lock();
    assert!(!state.grants.is_empty());
    let blocks = state.durable.free_blocks - state.reservations.outstanding() + 1;
    assert!(blocks <= state.durable.free_blocks);
    let (mut state, grant) = fs.reserve_with_pressure(CostOp::Unlink, state,
        || Ok(fs.inner.state.lock()), |s| s.reserve_blocks(blocks, Operation::Unlink)).unwrap();
    state.reservations.release(grant);
    drop(state);
    assert_eq!(attribution(), 1, "backstop publication must retain exact attribution");
    assert_eq!(rows, 900);
    assert_eq!(foreground, 0, "replay must not publish in foreground admission");
    assert_eq!(exceeded, 0, "outstanding exceeded trigger plus one worst-case operation grant");
    assert!(background > 0, "worker must publish before the periodic deadline");
}
