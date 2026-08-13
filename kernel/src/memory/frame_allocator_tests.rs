use super::*;
use crate::test_framework::registry::TestResult;

fn inject_duplicate_candidates(frame: PhysFrame, count: usize) -> bool {
    let mut free = FREE_FRAMES.lock();
    if free.try_reserve(count).is_err() {
        return false;
    }
    FREE_FRAME_CAPACITY.store(free.capacity(), Ordering::Release);
    for _ in 0..count {
        free.push(frame);
    }
    true
}

fn remove_duplicate_candidates(frame: PhysFrame) {
    FREE_FRAMES.lock().retain(|candidate| *candidate != frame);
}

fn republish_lost_frame(frame: PhysFrame) -> bool {
    let mut free = FREE_FRAMES.lock();
    if free.len() == free.capacity() {
        return false;
    }
    free.push(frame);
    true
}

pub(crate) fn republish_frame_for_gate(frame: PhysFrame) -> bool {
    republish_lost_frame(frame)
}

pub(crate) fn retire_with_free_list_contended(
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
    pid: u64,
    budget: &mut u32,
) -> crate::memory::process_memory::RetireProgress {
    let _free_list = FREE_FRAMES.lock();
    page_table.retire_bounded(pid, budget)
}

fn restore_lease(lease: FrameLease) -> bool {
    match return_lease(lease) {
        ReturnOutcome::Returned => true,
        ReturnOutcome::LostContended => republish_lost_frame(lease.frame),
        _ => false,
    }
}

fn free_frame_count(frame: PhysFrame) -> usize {
    FREE_FRAMES
        .lock()
        .iter()
        .filter(|candidate| **candidate == frame)
        .count()
}

pub fn free_list_len_for_gate() -> usize {
    FREE_FRAMES.lock().len()
}

fn take_free_frame(frame: PhysFrame) -> Option<FrameLease> {
    let candidate = {
        let mut free = FREE_FRAMES.lock();
        let index = free.iter().position(|candidate| *candidate == frame)?;
        free.swap_remove(index)
    };
    claim_frame(candidate).ok().flatten()
}

/// A frame above every usable region. This forces the production ordinal
/// lookup to reject the address rather than merely indexing past the ledger.
fn above_top_of_ram_frame() -> Option<PhysFrame> {
    let info = MEMORY_INFO.get()?;
    let top = info.regions[..info.region_count]
        .iter()
        .flatten()
        .map(|region| region.end)
        .max()?;
    let aligned = top.checked_add(0xfff)? & !0xfff;
    Some(PhysFrame::containing_address(PhysAddr::new(aligned)))
}

/// Atomically consume one sequential ordinal without claiming its ledger slot.
/// The successful frontier CAS makes the returned frame permanently never-issued,
/// so exercising the production deallocation wrapper cannot race a live owner.
fn reserve_never_allocated_frame() -> Option<PhysFrame> {
    loop {
        let index = NEXT_FREE_FRAME.load(Ordering::Acquire);
        let frame = BootInfoFrameAllocator::get_usable_frame(index)?;
        if !matches!(prepare_frame_for_allocation(index), PrepareFrame::Ready) {
            return None;
        }
        if NEXT_FREE_FRAME
            .compare_exchange(index, index + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Some(frame);
        }
    }
}

fn stale_lease_fixture() -> Option<(FrameLease, FrameLease)> {
    for _ in 0..3 {
        let stale = allocate_frame_leased()?;
        match return_lease(stale) {
            ReturnOutcome::Returned => {
                let current = take_free_frame(stale.frame)?;
                if current.index != stale.index || current.generation == stale.generation {
                    return None;
                }
                return Some((stale, current));
            }
            ReturnOutcome::LostContended => {
                if !republish_lost_frame(stale.frame) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    None
}

fn counters() -> [u64; 6] {
    use crate::tracing::providers::teardown;
    [
        teardown::FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        teardown::FRAME_RETURN_REFUSED_STALE.aggregate(),
        teardown::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        teardown::FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        teardown::FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        teardown::FRAME_LOST_CONTENDED.aggregate(),
    ]
}

fn healthy_round_trip() -> bool {
    let before = counters();
    let Some(lease) = allocate_frame_leased() else {
        return false;
    };
    return_lease(lease) == ReturnOutcome::Returned && counters()[..5] == before[..5]
}

pub fn frame_custody_refusal_gate_test() -> TestResult {
    let start = counters();
    if start[..5] != [0; 5] {
        return TestResult::Fail("A-E: unexpected refusal preceded injection gate");
    }

    // A: one committed return followed by one refused double return.
    let lease = match allocate_frame_leased() {
        Some(lease) => lease,
        None => return TestResult::Fail("A: lease allocation failed"),
    };
    let free_before = FREE_FRAMES.lock().len();
    if return_lease(lease) != ReturnOutcome::Returned
        || return_lease(lease) != ReturnOutcome::RefusedDoubleRelease
        || FREE_FRAMES.lock().len() != free_before + 1
        || free_frame_count(lease.frame) != 1
        || !healthy_round_trip()
    {
        return TestResult::Fail("A: double return or recovery was not exact");
    }

    // B: reuse the exact frame and prove the copied old lease is stale.
    let (stale, current) = match stale_lease_fixture() {
        Some(fixture) => fixture,
        None => return TestResult::Fail("B: exact frame reuse failed"),
    };
    if return_lease(stale) != ReturnOutcome::RefusedStale {
        return TestResult::Fail("B: stale lease was not refused");
    }
    let state = FRAME_LEDGER
        .get()
        .and_then(|ledger| ledger.get(current.index as usize))
        .map(|slot| slot.load(Ordering::Acquire));
    if state
        .is_none_or(|state| state & STATE_MASK != ST_ALLOCATED || state >> 2 != current.generation)
    {
        return TestResult::Fail("B: stale return changed the current owner");
    }
    if return_lease(current) != ReturnOutcome::Returned || !healthy_round_trip() {
        return TestResult::Fail("B: healthy return failed after stale refusal");
    }

    // C: out-of-region and in-region-never-issued frames are distinct.
    let untracked = match above_top_of_ram_frame() {
        Some(frame) => frame,
        None => return TestResult::Fail("C: above-RAM frame fixture unavailable"),
    };
    let free_before = FREE_FRAMES.lock().len();
    let before_untracked = counters();
    deallocate_frame(untracked);
    let free_after_untracked = FREE_FRAMES.lock().len();
    let after_untracked = counters();
    remove_duplicate_candidates(untracked);
    if after_untracked[3] != before_untracked[3] + 1 || free_after_untracked != free_before {
        return TestResult::Fail("C: untracked frame return was not isolated");
    }
    let never_frame = match reserve_never_allocated_frame() {
        Some(frame) => frame,
        None => return TestResult::Fail("C: no never-allocated frame available"),
    };
    let before_never = counters();
    deallocate_frame(never_frame);
    if counters()[2] != before_never[2] + 1
        || FREE_FRAMES.lock().len() != free_before
        || !healthy_round_trip()
    {
        return TestResult::Fail("C: refusal taxonomy or recovery was not exact");
    }

    // D: every injected live duplicate is withheld before escaping.
    let live = match allocate_frame_leased() {
        Some(lease) => lease,
        None => return TestResult::Fail("D: live lease allocation failed"),
    };
    let live_before = FRAME_LEDGER
        .get()
        .and_then(|ledger| ledger.get(live.index as usize))
        .map(|slot| slot.load(Ordering::Acquire));
    let duplicate_before = counters()[4];
    if !inject_duplicate_candidates(live.frame, 3) {
        let _ = restore_lease(live);
        return TestResult::Fail("D: duplicate fixture reservation failed");
    }
    let replacement = allocate_frame_leased();
    remove_duplicate_candidates(live.frame);
    let live_after = FRAME_LEDGER
        .get()
        .and_then(|ledger| ledger.get(live.index as usize))
        .map(|slot| slot.load(Ordering::Acquire));
    let replacement_is_distinct = replacement
        .as_ref()
        .is_some_and(|replacement| replacement.frame != live.frame);
    let replacement_missing = replacement.is_none();
    let replacement_returned = replacement.is_none_or(restore_lease);
    let live_returned = restore_lease(live);
    if !replacement_returned || !live_returned {
        return TestResult::Fail("D: failure cleanup could not restore frame owners");
    }
    if live_before.is_none()
        || live_after != live_before
        || !replacement_is_distinct
        || counters()[4] != duplicate_before + 3
    {
        if replacement_missing {
            return TestResult::Fail("D: duplicates were reported as OOM");
        }
        return TestResult::Fail("D: duplicate live frame escaped allocation");
    }
    if !healthy_round_trip() {
        return TestResult::Fail("D: healthy allocation failed after duplicate refusal");
    }

    // E: hold the real lock across a real return, then repair the lost frame.
    let contended = match allocate_frame_leased() {
        Some(lease) => lease,
        None => return TestResult::Fail("E: contended lease allocation failed"),
    };
    let free_guard = FREE_FRAMES.lock();
    let outcome = return_lease(contended);
    drop(free_guard);
    if outcome != ReturnOutcome::LostContended {
        return TestResult::Fail("E: real free-list contention was not reported");
    }
    let lost_state = FRAME_LEDGER
        .get()
        .and_then(|ledger| ledger.get(contended.index as usize))
        .map(|slot| slot.load(Ordering::Acquire));
    if lost_state.is_none_or(|state| state & STATE_MASK != ST_FREE)
        || free_frame_count(contended.frame) != 0
    {
        return TestResult::Fail("E: contended frame was not isolated as a loss");
    }
    let repaired = match claim_frame(contended.frame).ok().flatten() {
        Some(lease) => lease,
        None => return TestResult::Fail("E: lost-frame repair failed"),
    };
    let before_healthy = counters();
    if return_lease(repaired) != ReturnOutcome::Returned
        || counters()[..5] != before_healthy[..5]
        || !healthy_round_trip()
    {
        return TestResult::Fail("E: healthy return did not recover after contention");
    }

    let end = counters();
    if end[0] != start[0] + 1
        || end[1] != start[1] + 1
        || end[2] != start[2] + 1
        || end[3] != start[3] + 1
        || end[4] != start[4] + 3
        || end[5] < start[5] + 1
    {
        return TestResult::Fail("A-E: refusal counter deltas were not exact");
    }
    TestResult::Pass
}

pub fn frame_custody_healthy_counters_test() -> TestResult {
    if counters()[..5] != [1, 1, 1, 1, 3] {
        return TestResult::Fail("late guard read found an unexpected production refusal");
    }
    TestResult::Pass
}

#[cfg(target_arch = "x86_64")]
pub fn run_x86_frame_custody_gate() {
    crate::serial_println!("[TEST:process:frame_custody_refusal_gate:START]");
    let result = frame_custody_refusal_gate_test();
    if result.is_pass() {
        crate::serial_println!("[TEST:process:frame_custody_refusal_gate:PASS]");
    } else {
        crate::serial_println!(
            "[TEST:process:frame_custody_refusal_gate:FAIL:{:?}]",
            result
        );
    }
    let values = counters();
    crate::serial_println!(
        "[FRAME_CUSTODY_COUNTERS:x86:double={}:stale={}:never={}:untracked={}:duplicate={}:contended={}]",
        values[0], values[1], values[2], values[3], values[4], values[5]
    );
    assert!(result.is_pass(), "x86 frame custody refusal gate failed");
}
