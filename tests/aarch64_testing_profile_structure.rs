//! Structural regressions for the aarch64 `testing` profile (#562 and #761).
//!
//! These checks pin the cross-file contracts that failed at runtime. They are
//! intentionally about behavior-bearing call shapes rather than line numbers.

use std::fs;
use std::path::PathBuf;

fn repo_text(relative: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}");
    let start = source
        .find(&signature)
        .unwrap_or_else(|| panic!("find function {name}"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("find opening brace for {name}"));
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("find closing brace for {name}")
}

#[test]
fn softirq_overflow_uses_local_pinned_daemons() {
    let softirqd = repo_text("kernel/src/task/softirqd.rs");
    let wake = function_body(&softirqd, "wakeup_ksoftirqd");
    let init = function_body(&softirqd, "init_online_ksoftirqds");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");

    assert!(
        softirqd.contains("static KSOFTIRQD: [Once<KthreadHandle>; MAX_CPUS]"),
        "ksoftirqd handles must be per-CPU and lock-free to read at IRQ exit"
    );
    assert!(
        wake.contains("current_cpu_id()") && wake.contains("KSOFTIRQD[cpu].get()"),
        "softirq overflow must wake the executing CPU's daemon"
    );
    assert!(
        init.contains("kthread_run_on_cpu") && init.contains("for cpu in 0..online_cpu_count()"),
        "one CPU-pinned ksoftirqd must be created for every online CPU"
    );
    assert!(
        function_body(&scheduler, "find_target_cpu_for_wakeup").contains("cpu_affinity"),
        "wake routing must preserve production CPU affinity"
    );
}

#[test]
fn softirq_daemon_test_runs_outside_the_boot_idle_context() {
    let tests = repo_text("kernel/src/task/softirq_tests.rs");
    let wrapper = function_body(&tests, "test_softirq");
    let workload = function_body(&tests, "run_softirq_tests");

    assert!(
        wrapper.contains("kthread_run_on_cpu") && wrapper.contains("kthread_join"),
        "aarch64 must run the softirq test in a schedulable pinned kthread"
    );
    assert!(
        workload.contains("ksoftirqd should have processed deferred softirqs")
            && workload.contains("ksoftirqd should have processed deferred softirqs (tid={:?})"),
        "both the completion and daemon-identity assertions must remain"
    );
    assert!(
        workload.contains("crate::arch_without_interrupts(||"),
        "the bounded-call count must be sampled before the local daemon can race it"
    );
    assert!(
        wrapper.contains("SOFTIRQ_TEST: iteration limit passed"),
        "the aarch64 serial proof must be emitted after the test kthread joins"
    );
}

#[test]
fn boot_adds_softirq_daemons_after_secondary_cpus_are_online() {
    let main = repo_text("kernel/src/main_aarch64.rs");
    let smp_summary = main
        .find("[smp] {} CPUs online")
        .expect("SMP online summary marker");
    let add_daemons = main
        .find("init_online_ksoftirqds()")
        .expect("post-SMP ksoftirqd initialization");
    let softirq_test = main
        .find("softirq_tests::test_softirq()")
        .expect("softirq self-test call");

    assert!(
        smp_summary < add_daemons && add_daemons < softirq_test,
        "secondary ksoftirqd instances must exist before the testing-profile self-test"
    );
}
