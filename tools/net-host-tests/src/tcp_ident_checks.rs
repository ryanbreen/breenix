//! TCP initial sequence numbers (RFC 6528) and ephemeral ports (RFC 6056,
//! Algorithm 1), from the kernel's `net/tcp_ident.rs`.

use crate::tcp_ident::{ephemeral_candidates, isn, EPHEMERAL_FIRST, EPHEMERAL_LAST, ISN_SECRET_LEN};
use std::collections::BTreeSet;

const SECRET: [u8; ISN_SECRET_LEN] = [0x5a; ISN_SECRET_LEN];
const LOCAL: [u8; 4] = [10, 0, 2, 15];
const REMOTE: [u8; 4] = [10, 0, 2, 2];

/// A fixed pseudorandom sequence standing in for the kernel pool.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[test]
fn isns_differ_across_four_tuples_at_the_same_instant() {
    let now = 7_000_000;
    let base = isn(&SECRET, LOCAL, 40000, REMOTE, 22, now);
    // Each field of the 4-tuple, and the key, changes the result.
    assert_ne!(base, isn(&SECRET, [10, 0, 2, 16], 40000, REMOTE, 22, now));
    assert_ne!(base, isn(&SECRET, LOCAL, 40001, REMOTE, 22, now));
    assert_ne!(base, isn(&SECRET, LOCAL, 40000, [10, 0, 2, 3], 22, now));
    assert_ne!(base, isn(&SECRET, LOCAL, 40000, REMOTE, 23, now));
    assert_ne!(base, isn(&[0xa5; ISN_SECRET_LEN], LOCAL, 40000, REMOTE, 22, now));

    // 4096 connections opened at once from consecutive ports: the 4096 ISNs
    // are distinct, and the steps between neighbours vary (the counter this
    // replaced advanced by 64,000 per connection).
    let isns: Vec<u32> = (0..4096u16).map(|n| isn(&SECRET, LOCAL, 32768 + n, REMOTE, 80, now)).collect();
    assert_eq!(isns.iter().collect::<BTreeSet<_>>().len(), isns.len());
    let steps: BTreeSet<u32> = isns.windows(2).map(|w| w[1].wrapping_sub(w[0])).collect();
    assert!(steps.len() > 4000, "only {} distinct steps between neighbours", steps.len());
}

#[test]
fn an_isn_advances_with_the_four_microsecond_clock_for_one_four_tuple() {
    let at = |us: u64| isn(&SECRET, LOCAL, 40000, REMOTE, 22, us);
    let start = 1_000_000u64;
    let first = at(start);
    // RFC 6528 section 3: M is a timer that ticks once per 4 microseconds.
    assert_eq!(at(start + 3), first);
    assert_eq!(at(start + 4).wrapping_sub(first), 1);
    let mut previous = first;
    let mut now = start;
    for gap in [4u64, 40, 4_000, 1_000_000, 60_000_000, 3_600_000_000] {
        now += gap;
        let next = at(now);
        let advanced = next.wrapping_sub(previous);
        assert_eq!(u64::from(advanced), gap / 4, "after {gap} us");
        // Later in sequence space (RFC 9293 section 3.4: within half the space).
        assert!(advanced > 0 && advanced < 1 << 31, "after {gap} us");
        previous = next;
    }
}

#[test]
fn ephemeral_ports_are_random_in_range_and_cover_the_range() {
    let span = u32::from(EPHEMERAL_LAST - EPHEMERAL_FIRST) + 1;
    // One search visits each of the range's 28,232 ports once.
    let all: Vec<u16> = ephemeral_candidates(0x1234_5678_9abc_def0).collect();
    assert_eq!(all.len() as u32, span);
    assert_eq!(all.iter().collect::<BTreeSet<_>>().len() as u32, span);
    assert!(all.iter().all(|&p| (EPHEMERAL_FIRST..=EPHEMERAL_LAST).contains(&p)));

    // 2000 binds in a row, each taking the first free port of its own search.
    let mut state = 42u64;
    let mut taken = BTreeSet::new();
    let mut order = Vec::new();
    for _ in 0..2000 {
        let port = ephemeral_candidates(splitmix(&mut state))
            .find(|p| !taken.contains(p))
            .expect("a free port");
        assert!((EPHEMERAL_FIRST..=EPHEMERAL_LAST).contains(&port));
        taken.insert(port);
        order.push(port);
    }
    let sequential = |ports: &[u16]| ports.windows(2).filter(|w| w[1] == w[0].wrapping_add(1)).count();
    assert!(sequential(&order) < 20, "{} of 1999 binds took the next port", sequential(&order));
    // The check flags the sequential allocator this replaced.
    let old: Vec<u16> = (EPHEMERAL_FIRST..EPHEMERAL_FIRST + 2000).collect();
    assert_eq!(sequential(&old), 1999);
}
