#[test]
fn contended_flock_sleeps_on_the_files_wait_queue() {
    let source = std::fs::read_to_string("kernel/src/fs/flock.rs")
        .expect("flock has a per-file wait path");
    let ready = source.find("Attempt::Blocked =>").expect("handle contention");
    let wait = &source[ready..];
    let prepare = wait.find("self.waiters.prepare_to_wait_checked(").expect("publish on the per-file queue");
    let blocked = wait.find("ThreadState::BlockedOnIO").expect("publish scheduler blocked state");
    let recheck = wait.find("should_wait(").expect("recheck contention under the wait queue lock");
    let queued = wait.find("PrepareOutcome::Queued").expect("sleep after publishing");
    let signal = wait.find("check_signals_for_wait()").expect("check pending signals before sleeping");
    let sleep = wait.find("schedule_current_wait()").expect("schedule the blocked waiter");
    let cleanup = wait.find("self.waiters.finish_wait()").expect("unregister the waiter");
    assert!(prepare < blocked && blocked < recheck && recheck < queued);
    assert!(queued < signal && signal < sleep && sleep < cleanup);
    assert!(!wait[..cleanup].contains("spin_loop"));
    assert!(!wait[..cleanup].contains("yield_current"));
}
