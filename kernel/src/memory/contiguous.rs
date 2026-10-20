//! Sequential contiguous reservations, shared with the host regression tests.
use core::sync::atomic::{AtomicUsize, Ordering};

pub(super) fn allocate<F: Copy>(
    count: usize,
    out: &mut [Option<F>],
    frontier: &AtomicUsize,
    mut frame_at: impl FnMut(usize) -> Option<F>,
    address: impl Fn(F) -> u64,
    mut prepare: impl FnMut(usize, usize) -> bool,
    mut claim: impl FnMut(F) -> bool,
    mut release: impl FnMut(F),
) -> Option<F> {
    if count == 0 || out.len() < count {
        return None;
    }
    // Region boundaries make forward progress; contention re-reads the
    // frontier. Stop only at real exhaustion, not after an arbitrary number
    // of small regions preceding a satisfiable run.
    loop {
        let base = frontier.load(Ordering::SeqCst);

        // Measure how many frames at the frontier are physically adjacent.
        let first = frame_at(base)?;
        let mut run = 1usize;
        let mut prev = address(first);
        while run < count {
            let Some(frame) = frame_at(base + run) else {
                break;
            };
            let address = address(frame);
            if address != prev + 4096 {
                break;
            }
            prev = address;
            run += 1;
        }

        // Prepare ledger capacity before moving the frontier.
        if !prepare(base, run) {
            return None;
        }

        if frontier
            .compare_exchange(base, base + run, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Another allocation took the frontier; re-measure and retry.
            continue;
        }

        // The ordinals are reserved; take one ledger generation for each.
        let mut claimed = 0usize;
        while claimed < run {
            let Some(frame) = frame_at(base + claimed) else {
                break;
            };
            if !claim(frame) {
                break;
            }
            out[claimed] = Some(frame);
            claimed += 1;
        }

        if claimed == count {
            return Some(first);
        }

        // Short run (region boundary) or a refused claim: release the claimed
        // prefix through the fail-closed return path.
        for slot in out[..claimed].iter_mut() {
            if let Some(frame) = slot.take() {
                release(frame);
            }
        }

        if claimed != run {
            // A refusal means the ledger disagrees about ownership of a frontier
            // frame. Do not spin on it - fail the caller instead.
            return None;
        }
    }
}
