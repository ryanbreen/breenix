//! Execute the actual suite prerequisite decisions with boundary observations
//! that need not be available on the machine running the guest suites.
#[path = "../libs/libbreenix/src/suite_prerequisites.rs"]
mod prerequisites;

use prerequisites::*;

/// The suite runner truncates a FAIL message past 160 bytes.
fn fits(why: String) -> String {
    assert!(why.len() <= 160, "runner would truncate measurements ({} bytes): {why}", why.len());
    why
}

#[test]
fn cpu_coverage_requires_all_measurement_processors() {
    // The longest purpose a suite passes.
    let purpose = "the contending threads on processor 0 while the case runs on processor 1";
    for required in [2, 4] {
        for online in 0..required {
            let why = fits(cpus(online, required, purpose).unwrap_err());
            assert!(why.contains(&format!("online={online}")), "{why}");
            assert!(why.contains(&format!("required>={required}")), "{why}");
            assert!(why.ends_with(purpose), "{why}");
        }
        cpus(required, required, purpose).unwrap();
    }
}

#[test]
fn unavailable_priority_inheritance_is_unsuccessful() {
    for offered in [-1, 0] {
        let why = fits(priority_inheritance(offered).unwrap_err());
        assert!(why.contains(&format!("_SC_THREAD_PRIO_INHERIT={offered}")), "{why}");
    }
    priority_inheritance(200809).unwrap();
}

#[test]
fn argument_and_stack_controls_must_fit_before_asserting_limits() {
    // One 4096-byte string more than ARG_MAX holds, up to the 8 MiB bound.
    assert_eq!(argument_allocation(131072).unwrap(), 33);
    assert_eq!(argument_allocation(8388607).unwrap(), 2048);
    let why = fits(argument_allocation(8388608).unwrap_err());
    assert!(why.contains("ARG_MAX=8388608, needs 2049 x 4096 bytes"), "{why}");
    assert!(why.contains("allocation bound=8388608 bytes"), "{why}");
    fits(argument_allocation(u64::MAX).unwrap_err());

    let why = fits(stack(2097151, 1024, 256).unwrap_err());
    assert!(why.contains("RLIMIT_STACK=2097151"), "{why}");
    assert!(why.contains("1024 KiB control before testing the 256 KiB limit"), "{why}");
    assert!(stack(1048575, 512, 128).unwrap_err().contains("512 KiB control before testing the 128 KiB limit"));
    stack(2097152, 1024, 256).unwrap();
    stack(u64::MAX, 1024, 256).unwrap();
}

#[test]
fn invalid_semaphore_input_never_wraps_to_a_valid_value() {
    assert_eq!(invalid_semaphore_value(0).unwrap(), 1);
    assert_eq!(invalid_semaphore_value(32767).unwrap(), 32768);
    assert_eq!(invalid_semaphore_value(i64::from(u32::MAX) - 1).unwrap(), u32::MAX);
    for max in [i64::MIN, -1, i64::from(u32::MAX), i64::from(u32::MAX) + 1, i64::MAX] {
        let why = fits(invalid_semaphore_value(max).unwrap_err());
        assert!(why.contains(&format!("SEM_VALUE_MAX={max};")), "{why}");
        assert!(why.contains("fits unsigned int"), "{why}");
    }
}

#[test]
fn eager_allocation_cannot_pass_as_msync_writeback() {
    for stored in [0, 8, 15] {
        delayed_allocation(stored, 16).unwrap();
    }
    for stored in [16, 24, i64::MAX] {
        let why = fits(delayed_allocation(stored, 16).unwrap_err());
        assert!(why.contains(&format!("allocated {stored}/16 blocks")), "{why}");
        assert!(why.contains("cached readback insufficient"), "{why}");
    }
}

#[test]
fn no_idle_coverage_is_unsuccessful_even_with_good_scheduling_numbers() {
    for (idle, busy, wait, wait_idle, measured) in [
        (0, 600, 100, 0, false),
        (299, 301, 100, 100, true),
        (600, 0, 100, 0, false),
        (i64::MIN, i64::MAX, i64::MIN, i64::MIN, false),
    ] {
        let why = fits(idle_measurement(idle, busy, wait, wait_idle, measured).unwrap_err());
        assert!(why.contains(&format!("idle/busy={idle}/{busy}ms")), "{why}");
        assert!(why.contains(&format!("wait/idle={wait}/{wait_idle}ms")), "{why}");
        assert!(why.contains(&format!("spare={measured}")), "{why}");
    }
    idle_measurement(300, 300, 100, 100, true).unwrap();
    idle_measurement(600, 0, 100, 100, true).unwrap();
}
