//! Bounded receive draining with a shared request and claim.

/// Result of a bounded network RX poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    Drained,
    BudgetExhausted,
    InProgress,
}

#[cfg(any(target_arch = "x86_64", test))]
pub fn drain(
    requested: &core::sync::atomic::AtomicBool,
    draining: &core::sync::atomic::AtomicBool,
    budget: u32,
    mut receive: impl FnMut(&mut [u8]) -> Option<usize>,
    mut deliver: impl FnMut(&[u8]),
) -> PollOutcome {
    use core::sync::atomic::Ordering::SeqCst;

    // Request, then claim. The holder releases, then reads the request, so
    // with sequentially consistent ordering one of the two always sees the
    // other: either this claim succeeds or the holder drains again.
    // claim-lint:ok: SeqCst orders both flag operations; tests/net_rx_claim.rs exercises the request during a drain.
    requested.store(true, SeqCst);
    let mut buffer = [0u8; 2048];
    let mut remaining = budget;
    loop {
        if draining
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_err()
        {
            return PollOutcome::InProgress;
        }
        requested.store(false, SeqCst);

        while remaining > 0 {
            let Some(len) = receive(&mut buffer) else {
                break;
            };
            deliver(&buffer[..len]);
            remaining -= 1;
        }

        draining.store(false, SeqCst);
        if remaining == 0 {
            return PollOutcome::BudgetExhausted;
        }
        if !requested.load(SeqCst) {
            return PollOutcome::Drained;
        }
    }
}
