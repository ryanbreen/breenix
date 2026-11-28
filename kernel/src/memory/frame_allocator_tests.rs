use super::*;
use crate::test_framework::registry::TestResult;

/// Take the free list's frames off it, in list order. The buffer is reserved
/// before the list is held, so the hold makes no allocation.
fn drain_free_list() -> Option<Vec<u64>> {
    let meta = frame_meta()?;
    loop {
        let wanted = with_free_frames(|list| list.len()) + 64;
        let mut frames = Vec::new();
        frames.try_reserve_exact(wanted).ok()?;
        let drained = with_free_frames(|list| {
            if list.len() > frames.capacity() {
                return false;
            }
            while let Some(frame) = list.pop(meta) {
                frames.push(frame);
            }
            true
        });
        if drained {
            return Some(frames);
        }
    }
}

/// Put drained frames back in their original order, less `except`.
fn refill_free_list(frames: &[u64], except: Option<u64>) {
    let Some(meta) = frame_meta() else {
        return;
    };
    with_free_frames(|list| {
        for &frame in frames.iter().rev() {
            if Some(frame) != except {
                list.push(meta, frame);
            }
        }
    });
}

pub fn free_list_len_for_gate() -> usize {
    with_free_frames(|list| list.len())
}

fn free_frame_count(frame: PhysFrame) -> usize {
    let Some(frames) = drain_free_list() else {
        return usize::MAX;
    };
    let count = frames.iter().filter(|&&candidate| candidate == pfn(frame)).count();
    refill_free_list(&frames, None);
    count
}

/// Put live frames at the head of the free list, as a corrupt return would.
fn inject_live_candidates(frames: &[PhysFrame]) -> bool {
    let Some(meta) = frame_meta() else {
        return false;
    };
    with_free_frames(|list| frames.iter().all(|&frame| list.push(meta, pfn(frame))))
}

/// Remove any injected candidate the allocator has not consumed.
fn remove_candidates(frames: &[PhysFrame]) {
    let Some(drained) = drain_free_list() else {
        return;
    };
    let Some(meta) = frame_meta() else {
        return;
    };
    with_free_frames(|list| {
        for &frame in drained.iter().rev() {
            if !frames.iter().any(|&injected| pfn(injected) == frame) {
                list.push(meta, frame);
            }
        }
    });
}

fn take_free_frame(frame: PhysFrame) -> Option<FrameLease> {
    let frames = drain_free_list()?;
    let found = frames.contains(&pfn(frame));
    refill_free_list(&frames, Some(pfn(frame)));
    if !found {
        return None;
    }
    claim_frame(frame, 0, 0)
}

/// The first frame above the highest usable range. This forces the
/// production lookup to reject the address rather than index past the
/// metadata.
fn above_top_of_ram_frame() -> Option<PhysFrame> {
    let info = MEMORY_INFO.get()?;
    let top = info.iter()
        .map(|region| region.end)
        .max()?;
    let aligned = top.checked_add(0xfff)? & !0xfff;
    Some(PhysFrame::containing_address(PhysAddr::new(aligned)))
}

/// Atomically consume one sequential ordinal without claiming its metadata.
/// The successful frontier CAS makes the returned frame permanently never-issued,
/// so exercising the production deallocation wrapper cannot race a live owner.
fn reserve_never_allocated_frame() -> Option<PhysFrame> {
    loop {
        let index = NEXT_FREE_FRAME.load(Ordering::Acquire);
        let frame = BootInfoFrameAllocator::get_usable_frame(index)?;
        if NEXT_FREE_FRAME
            .compare_exchange(index, index + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Some(frame);
        }
    }
}

fn stale_lease_fixture() -> Option<(FrameLease, FrameLease)> {
    let stale = allocate_frame_leased()?;
    if return_lease(stale) != ReturnOutcome::Returned {
        return None;
    }
    let current = take_free_frame(stale.frame)?;
    if current.frame != stale.frame || current.generation == stale.generation {
        return None;
    }
    Some((stale, current))
}

fn counters() -> [u64; 5] {
    use crate::tracing::providers::teardown;
    [
        teardown::FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        teardown::FRAME_RETURN_REFUSED_STALE.aggregate(),
        teardown::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        teardown::FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        teardown::FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
    ]
}

fn healthy_round_trip() -> bool {
    let before = counters();
    let Some(lease) = allocate_frame_leased() else {
        return false;
    };
    return_lease(lease) == ReturnOutcome::Returned && counters() == before
}

fn frame_state(frame: PhysFrame) -> Option<(u32, u32)> {
    let entry = frame_meta()?.entry(pfn(frame))?;
    Some((entry.state(), entry.generation()))
}

pub fn frame_custody_refusal_gate_test() -> TestResult {
    let start = counters();
    if start != [0; 5] {
        return TestResult::Fail("A-D: unexpected refusal preceded injection gate");
    }

    // A: one committed return followed by one refused double return.
    let lease = match allocate_frame_leased() {
        Some(lease) => lease,
        None => return TestResult::Fail("A: lease allocation failed"),
    };
    let free_before = free_list_len_for_gate();
    if return_lease(lease) != ReturnOutcome::Returned
        || return_lease(lease) != ReturnOutcome::RefusedDoubleRelease
        || free_list_len_for_gate() != free_before + 1
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
    if frame_state(current.frame) != Some((ST_ALLOCATED, current.generation)) {
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
    let free_before = free_list_len_for_gate();
    let before_untracked = counters();
    deallocate_frame(untracked);
    let after_untracked = counters();
    if after_untracked[3] != before_untracked[3] + 1 || free_list_len_for_gate() != free_before {
        return TestResult::Fail("C: untracked frame return was not isolated");
    }
    let never_frame = match reserve_never_allocated_frame() {
        Some(frame) => frame,
        None => return TestResult::Fail("C: no never-allocated frame available"),
    };
    let before_never = counters();
    deallocate_frame(never_frame);
    if counters()[2] != before_never[2] + 1
        || free_list_len_for_gate() != free_before
        || !healthy_round_trip()
    {
        return TestResult::Fail("C: refusal taxonomy or recovery was not exact");
    }

    // D: three live frames put on the free list are each withheld before
    // escaping, and the allocation still succeeds with a distinct frame.
    let mut live = [None; 3];
    for slot in live.iter_mut() {
        *slot = allocate_frame_leased();
    }
    let [Some(first), Some(second), Some(third)] = live else {
        for lease in live.into_iter().flatten() {
            let _ = return_lease(lease);
        }
        return TestResult::Fail("D: live lease allocation failed");
    };
    let live = [first, second, third];
    let frames = live.map(FrameLease::frame);
    let live_before = frames.map(frame_state);
    let duplicate_before = counters()[4];
    if !inject_live_candidates(&frames) {
        remove_candidates(&frames);
        live.into_iter().for_each(|lease| { let _ = return_lease(lease); });
        return TestResult::Fail("D: duplicate fixture injection failed");
    }
    let replacement = allocate_frame_leased();
    remove_candidates(&frames);
    let live_after = frames.map(frame_state);
    let replacement_is_distinct = replacement
        .as_ref()
        .is_some_and(|replacement| !frames.contains(&replacement.frame));
    let replacement_missing = replacement.is_none();
    let replacement_returned =
        replacement.is_none_or(|lease| return_lease(lease) == ReturnOutcome::Returned);
    let live_returned = live
        .into_iter()
        .all(|lease| return_lease(lease) == ReturnOutcome::Returned);
    if !replacement_returned || !live_returned {
        return TestResult::Fail("D: failure cleanup could not restore frame owners");
    }
    if live_before.contains(&None)
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

    let end = counters();
    if end[0] != start[0] + 1
        || end[1] != start[1] + 1
        || end[2] != start[2] + 1
        || end[3] != start[3] + 1
        || end[4] != start[4] + 3
    {
        return TestResult::Fail("A-D: refusal counter deltas were not exact");
    }
    TestResult::Pass
}

pub fn frame_custody_healthy_counters_test() -> TestResult {
    if counters() != [1, 1, 1, 1, 3] {
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
        "[FRAME_CUSTODY_COUNTERS:x86:double={}:stale={}:never={}:untracked={}:duplicate={}]",
        values[0], values[1], values[2], values[3], values[4]
    );
    assert!(result.is_pass(), "x86 frame custody refusal gate failed");
}
