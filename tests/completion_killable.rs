//! Pin the killable completion's signal checks and sleep publication.
const COMPLETION: &str = include_str!("../kernel/src/task/completion.rs");
const SYSCALL: &str = include_str!("../kernel/src/syscall/mod.rs");

#[test]
fn killable_wait_checks_fatal_signal_at_each_syscall_wait_boundary() {
    assert!(COMPLETION.contains("pub fn wait_timeout_killable("));
    let inner = COMPLETION.split("fn wait_timeout_inner(").nth(1).unwrap()
        .split("pub fn complete(").next().unwrap();
    assert_eq!(inner.matches("wait_mode.signal_pending()").count(), 3);
    let after_store = inner.find("// Fast check after waiter store").unwrap();
    let enable = inner.find("// Enable preemption").unwrap();
    let block = inner.find("let already_done =").unwrap();
    let wake = inner.find("// Clear BlockedOnIO state after wake").unwrap();
    let checks: Vec<_> = inner.match_indices("wait_mode.signal_pending()").map(|(i, _)| i).collect();
    assert!(after_store < checks[0] && checks[0] < enable);
    assert!(enable < checks[1] && checks[1] < block);
    assert!(wake < checks[2]);
    assert!(inner.contains("thread.uninterruptible_io = wait_mode == WaitMode::Uninterruptible"));
    assert!(COMPLETION.contains("WaitMode::Killable => crate::syscall::fatal_signal_pending()"));
    assert!(SYSCALL.contains("pub fn fatal_signal_pending()"));
    assert!(SYSCALL.contains("signals.mirror.pending() & crate::signal::constants::sig_mask(crate::signal::constants::SIGKILL) != 0"));
}
