use guest_budget::{extension_bound_ms, grant_extension, ExtensionPolicy, POLICY};

// Mutation exists only in the host fixture; it cannot enable a kernel bypass.
const ALWAYS_EXTEND: bool = false;
fn grant(ctx: u64, elapsed: u64, used: u64, policy: ExtensionPolicy) -> u64 {
    let always_extend = ALWAYS_EXTEND;
    if always_extend { 100 } else { grant_extension(ctx, elapsed, used, policy) }
}

#[test]
fn policy_boundaries() {
    for (ctx, elapsed, expected) in [
        (29, 200, 0), (0, u64::MAX, 0), (30, 149, 0),
        (30, 150, 50), (30, 180, 50), (30, 181, 100),
        (u64::MAX, u64::MAX, 100),
    ] {
        assert_eq!(grant(ctx, elapsed, 0, POLICY), expected, "ctx={ctx} elapsed={elapsed}");
    }
    assert_eq!(grant(30, 224, 100, POLICY), 0);
    assert_eq!(grant(30, 225, 100, POLICY), 50);
    assert_eq!(grant(30, 270, 100, POLICY), 50);
    assert_eq!(grant(30, 271, 100, POLICY), 100);
    assert_eq!(grant(30, 100, 0, ExtensionPolicy { budget_ms: 0, ..POLICY }), 0);
}

#[test]
fn policy_cap() {
    assert_eq!(extension_bound_ms(POLICY), 400);
    assert_eq!(grant(30, 1000, 175, POLICY), 25);
    assert_eq!(grant(30, 1000, 200, POLICY), 0);
    assert_eq!(grant(30, 1000, u64::MAX, POLICY), 0);
    let oversized = ExtensionPolicy { max_extension_ms: u64::MAX, ..POLICY };
    assert_eq!(extension_bound_ms(oversized), 400);
    assert_eq!(grant(30, 1000, 200, oversized), 0);
    let limited = ExtensionPolicy { max_extension_ms: 25, ..POLICY };
    assert_eq!(grant(30, 200, 0, limited), 25);
    assert_eq!(extension_bound_ms(limited), 225);
}

// Deterministic scheduling fixture, not a captured QEMU execution. At 200 ms
// the reader is runnable, has not run, and tick/counter = 40/200 is starved.
// The guest has made 30 context switches. The reader runs at 240 ms and
// receives three bytes if it was woken. Extensions never manufacture a wake.
fn replay(policy: ExtensionPolicy, runnable: bool, ready_ms: u64, bytes: u64) -> (u64, bool) {
    let mut elapsed = policy.budget_ms;
    let mut extensions = 0;
    for _ in 0..10 {
        if runnable && elapsed >= ready_ms {
            return (extensions, bytes == 3);
        }
        let extra = grant(30, elapsed, extensions, policy);
        if !runnable || extra == 0 { return (extensions, false); }
        extensions += extra;
        assert!(extensions <= 200, "extension hard cap exceeded");
        elapsed += extra;
    }
    panic!("extension loop did not terminate");
}

#[test]
fn starved_recovery() {
    let disabled = ExtensionPolicy { max_extension_ms: 0, ..POLICY };
    assert!(loopback_window_starved(40, 200, 30));
    assert_eq!(replay(disabled, true, 240, 3), (0, false));
    println!("fixture: before verdict=starved extensions=0 FAIL");
    assert_eq!(replay(POLICY, true, 240, 3), (100, true));
    println!("fixture: after verdict=starved extensions=100 PASS");
}

#[test]
fn extension_does_not_hide_wake_or_receive_defects() {
    assert_eq!(replay(POLICY, false, 240, 3), (0, false));
    assert_eq!(replay(POLICY, true, 240, 2), (100, false));
    assert_eq!(replay(POLICY, true, u64::MAX, 3), (200, false));
}
