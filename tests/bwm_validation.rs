#[path = "../userspace/programs/src/bwm/chrome/validation.rs"]
mod validation;

use validation::ValidationQueue;

#[test]
fn latest_scene_and_one_first_frame_per_live_client() {
    let mut queue = ValidationQueue::new();
    for serial in 1..=1000 {
        queue.presented(serial);
        queue.first_frame(7);
        queue.first_frame(8);
    }
    queue.retain_clients(|id| id == 8);
    let (scene, clients) = queue.next_if_idle(|_| true, || true).unwrap();
    assert_eq!(*scene, 1000, "only the newest full scene remains");
    assert_eq!(clients, [8], "duplicates and departed clients leave no work");
    queue.complete(8);
    assert!(queue.next_if_idle(|_| true, || true).is_none());

    // A newly eligible first frame can use the last presented scene without
    // requiring another presentation just to schedule validation.
    queue.first_frame(9);
    assert!(queue.next_if_idle(|_| false, || true).is_none());
    let (scene, clients) = queue.next_if_idle(|_| true, || true).unwrap();
    assert_eq!((*scene, clients), (1000, vec![9]));
    queue.invalidate_scene();
    assert!(queue.next_if_idle(|_| true, || true).is_none());
    queue.presented(1001);
    assert_eq!(queue.next_if_idle(|_| true, || true).unwrap().1, [9]);
    queue.complete(9);
    assert!(queue.next_if_idle(|_| true, || true).is_none());
}

#[test]
fn ready_input_or_scene_defers_work_and_idle_takes_one_check() {
    let mut queue = ValidationQueue::new();
    queue.presented(42);
    queue.first_frame(1);
    queue.first_frame(2);
    for ready in [[true, false], [false, true], [true, true]] {
        assert!(queue.next_if_idle(|_| true, || !ready.iter().any(|r| *r)).is_none());
    }
    let mut polls = 0;
    let (scene, clients) = queue.next_if_idle(|_| true, || { polls += 1; true }).unwrap();
    assert_eq!((*scene, clients), (42, vec![1, 2]));
    assert_eq!(polls, 1);
    // One successful full-scene comparison serves both clients.
    queue.complete(1);
    queue.complete(2);
    assert!(queue.next_if_idle(|_| true, || panic!("no work to poll for")).is_none());
}

#[test]
fn production_loop_acknowledges_before_idle_validation() {
    let main = include_str!("../userspace/programs/src/bwm.rs");
    let chrome = include_str!("../userspace/programs/src/bwm/chrome.rs");
    let loop_body = &main[main.find("    loop {\n        flush_input();").unwrap()..];
    let mut tail = loop_body;
    for part in ["io::poll(&mut poll_fds, timeout)", "if stdin_ready", "gpu.compose(",
        "clock.acknowledgment(claim_epoch)", "op::COMPOSITOR_ACK", "run_deferred_check("] {
        let at = tail.find(part).unwrap_or_else(|| panic!("missing or out of order: {part}"));
        tail = &tail[at + part.len()..];
    }
    assert!(!loop_body.contains("gpu.check("), "presentation must only enqueue checks");
    assert_eq!(loop_body.matches("run_deferred_check(").count(), 1);
    let idle = &chrome[chrome.find("fn validation_idle").expect("idle descriptor poll")..];
    let idle = &idle[..idle.find("\n}").unwrap()];
    assert!(idle.contains("io::poll(poll_fds, 0)"));
    assert!(idle.contains("fd.revents == 0"));
    assert!(idle.contains("Ok(0)"), "poll errors must defer validation");
    assert!(chrome.contains("mod validation;"), "chrome owns the queue");
    assert!(chrome.contains("VALUE bwm-check-us"));
}

#[test]
fn idle_presentations_without_pending_first_frames_do_not_read_back() {
    let mut queue = ValidationQueue::new();
    for frame in 0..10 {
        queue.presented(frame);
        assert!(queue.next_if_idle(|_| true, || panic!("no pending client")).is_none());
    }
    queue.first_frame(1);
    assert_eq!(queue.next_if_idle(|_| true, || true).unwrap().1, [1]);
    assert!(queue.next_if_idle(|_| true, || panic!("already attempted")).is_none());
    queue.presented(10);
    assert_eq!(queue.next_if_idle(|_| true, || true).unwrap().1, [1]);
    queue.complete(1);
    queue.presented(11);
    assert!(queue.next_if_idle(|_| true, || panic!("client completed")).is_none());
    queue.first_frame(2);
    queue.retain_clients(|_| false);
    assert!(queue.next_if_idle(|_| true, || panic!("client departed")).is_none());
}
