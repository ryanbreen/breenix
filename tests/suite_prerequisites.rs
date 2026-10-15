//! Execute the actual suite prerequisite decisions with boundary observations
//! that need not be available on the machine running the guest suites.
#[path = "../libs/libbreenix/src/suite_prerequisites.rs"]
mod prerequisites;

use prerequisites::*;

#[test]
fn cpu_coverage_requires_all_measurement_processors() {
    for required in [2, 4] {
        for online in 0..required {
            let why = cpus(online, required).unwrap_err();
            assert!(why.contains(&format!("{online} processors online")), "{why}");
            assert!(why.contains(&format!("at least {required}")), "{why}");
        }
        cpus(required, required).unwrap();
    }
}

#[test]
fn unavailable_priority_inheritance_is_unsuccessful() {
    for offered in [-1, 0] {
        let why = priority_inheritance(offered).unwrap_err();
        assert!(why.contains("_SC_THREAD_PRIO_INHERIT"), "{why}");
        assert!(why.contains(&format!("returned {offered}")), "{why}");
    }
    priority_inheritance(200809).unwrap();
}

#[test]
fn argument_and_stack_controls_must_fit_before_asserting_limits() {
    assert!(long_argument(131071).unwrap_err().contains("ARG_MAX=131071"));
    long_argument(131072).unwrap();
    argument_allocation(8388607, 2048).unwrap();
    assert!(argument_allocation(8388608, 2049).unwrap_err().contains("8388608-byte allocation bound"));
    assert!(stack(2097151, 1024).unwrap_err().contains("RLIMIT_STACK=2097151"));
    stack(2097152, 1024).unwrap();
    stack(u64::MAX, 1024).unwrap();
}

#[test]
fn invalid_semaphore_input_never_wraps_to_a_valid_value() {
    assert_eq!(invalid_semaphore_value(32767).unwrap(), 32768);
    assert_eq!(invalid_semaphore_value(i64::from(u32::MAX) - 1).unwrap(), u32::MAX);
    for max in [i64::from(u32::MAX), i64::from(u32::MAX) + 1] {
        let why = invalid_semaphore_value(max).unwrap_err();
        assert!(why.contains(&format!("SEM_VALUE_MAX={max}")), "{why}");
        assert!(why.contains("representable by unsigned int"), "{why}");
    }
}

#[test]
fn eager_allocation_cannot_pass_as_msync_writeback() {
    for stored in [0, 8, 15] {
        delayed_allocation(stored, 16).unwrap();
    }
    for stored in [16, 24] {
        let why = delayed_allocation(stored, 16).unwrap_err();
        assert!(why.contains(&format!("allocated {stored} blocks of 16")), "{why}");
        assert!(why.contains("cached readback cannot prove writeback"), "{why}");
    }
}

#[test]
fn no_idle_coverage_is_unsuccessful_even_with_good_scheduling_numbers() {
    for (idle, busy, wait, wait_idle, measured) in [
        (0, 600, 100, 0, false),
        (299, 301, 100, 100, true),
        (600, 0, 100, 0, false),
    ] {
        let why = idle_measurement(idle, busy, wait, wait_idle, measured).unwrap_err();
        assert!(why.contains(&format!("for {idle} ms")), "{why}");
        assert!(why.contains(&format!("contended for {busy} ms")), "{why}");
        assert!(why.contains(&format!("time={wait_idle} ms")), "{why}");
    }
    idle_measurement(300, 300, 100, 100, true).unwrap();
    idle_measurement(600, 0, 100, 100, true).unwrap();
}
