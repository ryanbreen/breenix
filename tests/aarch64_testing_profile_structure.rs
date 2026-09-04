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
        wake.contains("current_cpu_id()") && wake.contains("KSOFTIRQD.get(current_cpu_id())"),
        "softirq overflow must wake the executing CPU's daemon, through a bounds-checked index"
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
    let dispatch = function_body(&tests, "run_ksoftirqd_deferral_phase");
    let phase = function_body(&tests, "ksoftirqd_deferral_phase");
    let report = function_body(&tests, "report_ksoftirqd_deferral_phase");
    let workload = function_body(&tests, "run_softirq_tests");

    assert!(
        dispatch.contains("kthread_run_on_cpu") && dispatch.contains("kthread_join"),
        "aarch64 must run the daemon-verification phase in a schedulable pinned kthread"
    );
    assert!(
        phase.contains("ksoftirqd should have processed deferred softirqs")
            && phase.contains("ksoftirqd should have processed deferred softirqs (tid={:?})"),
        "both the completion and daemon-identity assertions must remain"
    );
    assert!(
        phase.contains("crate::arch_without_interrupts(||"),
        "the bounded-call count must be sampled before the local daemon can race it"
    );
    assert!(
        report.contains("SOFTIRQ_TEST: iteration limit passed")
            && report.contains("KSOFTIRQD_OBSERVED_CPU"),
        "the serial proof must name the daemon that was observed, not the pin"
    );
    assert!(
        workload.contains("run_ksoftirqd_deferral_phase()")
            && workload.contains("Test 1: Register handlers")
            && workload.contains("Test 8: Verify ksoftirqd is initialized"),
        "only the daemon-verification phase moves off the boot context"
    );
}

#[test]
fn softirq_handler_reads_its_identity_without_the_scheduler_lock() {
    let tests = repo_text("kernel/src/task/softirq_tests.rs");
    let phase = function_body(&tests, "ksoftirqd_deferral_phase");

    assert!(
        phase.contains("crate::per_cpu::current_thread_id_lock_free()"),
        "the softirq handler must read its identity from per-CPU state"
    );
    // The scheduler-lock read must be gone from the executable text. The
    // comment that explains WHY names the call it replaced, so the check reads
    // the handler's code rather than the whole phase's prose.
    let handler_start = phase
        .find("register_softirq_handler(SoftirqType::Tasklet")
        .expect("find the self-re-raising handler");
    let handler = &phase[handler_start..];
    let code: String = handler
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("scheduler::current_thread_id()"),
        "the softirq handler runs on the IRQ-exit path and must take no scheduler lock"
    );
}

#[test]
fn the_boot_sequence_runs_in_a_kernel_thread_and_the_loader_with_it() {
    let main = repo_text("kernel/src/main_aarch64.rs");
    let entry = function_body(&main, "kernel_main");
    let body = function_body(&main, "boot_continuation");
    assert!(
        entry.contains("kthread_run(") && entry.contains("boot_continuation("),
        "kernel_main must hand the rest of the boot sequence to a kernel thread"
    );
    assert!(
        !entry.contains("kthread_join"),
        "the idle identity must join nothing"
    );
    assert!(
        entry.contains("preempt_enable()"),
        "the boot pin must be released once the boot sequence is a schedulable thread"
    );
    let load = body
        .find("load_test_binaries_from_ext2();")
        .expect("the loader runs on the boot continuation");
    let marker = body
        .find("[test] Test processes loaded - will run via timer interrupts")
        .expect("the completion marker follows the loader");
    assert!(load < marker, "the loader must complete before its marker");
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

#[test]
fn testing_fork_inherits_cpu_affinity() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let fork = function_body(&manager, "fork_process_with_context");

    assert!(
        fork.contains("cpu_affinity: parent_thread.cpu_affinity"),
        "a fork child must preserve any scheduler affinity held by its parent"
    );
}

#[test]
fn completion_sleep_rejects_idle_masked_and_interrupt_contexts() {
    let completion = repo_text("kernel/src/task/completion.rs");
    let predicate = function_body(&completion, "current_context_can_sleep");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let idle_probe = function_body(&scheduler, "is_current_idle_thread");

    for required in [
        "crate::arch_interrupts_enabled()",
        "!crate::per_cpu_aarch64::in_interrupt()",
        "!crate::per_cpu_aarch64::in_softirq()",
        "crate::per_cpu_aarch64::preempt_count() == 1",
        "timer_interrupt::is_initialized()",
        // The idle test now lives in the one shared refusal, which owns the
        // counter and the marker as well as the verdict.
        "idle_sleep::idle_identity_must_not_sleep()",
    ] {
        assert!(
            predicate.contains(required),
            "completion sleep eligibility lost {required}"
        );
    }
    assert!(
        idle_probe.contains("current_thread_id_inner()")
            && !idle_probe.contains("unwrap_or(false)"),
        "missing scheduler state must not be classified as a non-idle task"
    );
}

#[test]
fn block_mmio_rejects_masked_irq_before_request_publication() {
    let block = repo_text("kernel/src/drivers/virtio/block_mmio.rs");
    let available = function_body(&block, "irq_completion_available");
    let gate_sleep = function_body(&block, "block_mmio_request_gate_can_sleep");
    let read = function_body(&block, "read_sector");

    assert!(
        available.contains("Aarch64Cpu::interrupts_enabled()")
            && !available.contains("current_thread_id"),
        "a current idle identity must not make masked-IRQ completion available"
    );
    assert!(
        gate_sleep.contains("completion::current_context_can_sleep()"),
        "gate waits and completion waits must share one sleep policy"
    );
    let eligibility = read.find("irq_completion_available()").unwrap();
    let gate = read.find("REQUEST_GATES[device_index].lock()").unwrap();
    let prepare = read.find("completion.prepare_wait()").unwrap();
    let publish = read.find("submit_read_sector(").unwrap();
    assert!(
        eligibility < gate && gate < prepare && prepare < publish,
        "masked IRQs must be rejected before gate acquisition and queue publication"
    );
}

#[test]
fn testing_loader_keeps_irqs_enabled_for_virtio_completion() {
    let main = repo_text("kernel/src/main_aarch64.rs");
    let loader = function_body(&main, "load_test_binaries_from_ext2");

    assert!(
        !loader.contains("disable_interrupts()"),
        "the IRQ-driven ext2 loader must not mask its completion interrupt"
    );
    assert!(
        loader.contains("Aarch64Cpu::enable_interrupts()"),
        "the loader must undo the test suite's deliberate IRQ mask before ext2 I/O"
    );
    assert!(
        loader.contains("root_fs_read()") && loader.contains("read_file_content(&inode)"),
        "the ratchet must cover the ext2-backed loader rather than a bypass"
    );
    let read = loader.find("read_file_content(&inode)").unwrap();
    let batch = loader.find("loaded_images.push((name, elf_data))").unwrap();
    let stage = loader.find("begin_test_binary_staging()").unwrap();
    let create = loader.find("for (name, elf_data) in loaded_images").unwrap();
    assert!(
        read < batch && batch < stage && stage < create,
        "all ext2 reads must finish before any test process becomes runnable"
    );

    let complete = main
        .find("[test] Test processes loaded - will run via timer interrupts")
        .unwrap();
    let release = main.find("finish_test_binary_staging()").unwrap();
    assert!(
        complete < release,
        "the complete process catalog must be reported before SMP dispatch starts"
    );
}
