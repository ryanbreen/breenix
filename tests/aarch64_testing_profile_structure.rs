//! Structural regressions for the aarch64 `testing` profile (#562 and #761).
//!
//! These checks pin the cross-file contracts that failed at runtime. They are
//! intentionally about behavior-bearing call shapes rather than line numbers.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

/// Every Rust source below `relative`, as (repo-relative path, contents).
/// claim-lint:ok: the walk is the same one `tests/teardown_structure.rs` uses
fn rust_sources_below(relative: &str) -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read source directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("source below repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, fs::read_to_string(path).expect("read Rust source")));
            }
        }
    }

    let root = repo_root();
    let mut sources = Vec::new();
    visit(&root, &root.join(relative), &mut sources);
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
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

// ---------------------------------------------------------------------------
// TTBR0 install census (#786)
// ---------------------------------------------------------------------------
//
// The per-CPU words `saved_process_cr3` (offset 80) and `next_cr3` (offset 64)
// are not bookkeeping: the syscall return corridor in `syscall_entry.S`
// installs `next_cr3` when it is non-zero and otherwise restores
// `saved_process_cr3`, and `is_ttbr0_root_live_in_mask` reads both before a
// page-table root may be reclaimed. A site that writes TTBR0_EL1 with a raw
// `msr` and leaves those words naming a different root does not merely
// disagree with the register -- it decides which root the next return to EL0
// runs on.
// claim-lint:ok: the corridor reads are cited in
// docs/planning/green-program/aarch64-testing/786-RCA-2026-09-04.md
//
// The census below walks the Rust functions under `kernel/src` that write
// TTBR0_EL1 -- its coverage floor is documented on `ttbr0_install_census` -- and
// sorts them by shape, not by a list of known sites:
//
//   * the discipline itself (`arch_impl/aarch64/ttbr0.rs`);
//   * functions that reconcile inline (they name `set_saved_process_cr3`);
//   * mechanism primitives, whose installed value traces back to one of their
//     own parameters -- they install what a caller decided, so the caller owns
//     the shadows;
//   * everything else, which must be empty outside the files CLAUDE.md lists
//     as Tier-1 prohibited.
// claim-lint:ok: 7 censused functions at this head, enumerated in
// docs/planning/green-program/aarch64-testing/786-RCA-2026-09-04.md

/// Files CLAUDE.md forbids modifying without explicit user approval. Checked
/// back against CLAUDE.md by `the_tier_one_exemption_matches_the_project_rule`
/// so this stays the project's rule rather than this test's opinion.
const TIER_ONE_PROHIBITED: [&str; 3] = [
    "kernel/src/syscall/handler.rs",
    "kernel/src/syscall/time.rs",
    "kernel/src/interrupts/timer.rs",
];

fn identifiers(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            out.insert(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.insert(current);
    }
    out
}

/// The right-hand side of the first `let <name> = ...;` binding in `body`.
fn let_binding_rhs(body: &str, name: &str) -> Option<String> {
    for prefix in [format!("let {name} = "), format!("let mut {name} = ")] {
        if let Some(start) = body.find(&prefix) {
            let rest = &body[start + prefix.len()..];
            let end = rest.find(';').unwrap_or(rest.len());
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Whether `operand` traces back, through this function's own `let` bindings,
/// to something the function was handed. Depth-limited: a mechanism primitive
/// masks or tags a parameter at most a couple of steps before installing it.
fn traces_to_a_parameter(signature: &str, body: &str, operand: &str) -> bool {
    let params = identifiers(signature);
    let mut frontier = vec![operand.to_string()];
    let mut seen = BTreeSet::new();
    for _ in 0..4 {
        let mut next = Vec::new();
        for name in std::mem::take(&mut frontier) {
            if params.contains(&name) {
                return true;
            }
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(rhs) = let_binding_rhs(body, &name) {
                next.extend(identifiers(&rhs));
            }
        }
        if next.is_empty() {
            return false;
        }
        frontier = next;
    }
    false
}

/// Every function name declared in `source`, in declaration order.
/// claim-lint:ok: the coverage floor asserted by `ttbr0_install_census` is what
/// measures this, 7 of 7 at this head
fn declared_function_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let rest = match trimmed.split_once("fn ") {
            Some((before, rest)) if before.is_empty() || before.ends_with(' ') => rest,
            _ => continue,
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

struct TtbrInstall {
    file: String,
    function: String,
    signature: String,
    body: String,
}

/// The declaration line of `name` in `source`.
fn function_signature(source: &str, name: &str) -> String {
    let needle = format!("fn {name}");
    for line in source.lines() {
        if line.contains(&needle) {
            return line.to_string();
        }
    }
    panic!("find signature for {name}")
}

/// Every function under `kernel/src` whose body writes TTBR0_EL1.
/// claim-lint:ok: 7 censused functions at this head, enumerated in
/// docs/planning/green-program/aarch64-testing/786-RCA-2026-09-04.md
///
/// The walk is name-driven, so what it carries is a coverage FLOOR: the install
/// occurrences inside censused bodies must be at least as many as the file
/// holds. Nested functions can be double-counted, so the floor does not by
/// itself exclude one hidden site paired with one double-count; what it does
/// catch is a file whose installs the name walk missed outright.
/// claim-lint:ok: 7 of 7 censused at this head, enumerated in
/// docs/planning/green-program/aarch64-testing/786-RCA-2026-09-04.md
fn ttbr0_install_census(sources: &[(String, String)]) -> Vec<TtbrInstall> {
    const INSTALL: &str = "msr ttbr0_el1";
    let mut out = Vec::new();
    for (file, source) in sources {
        let total = source.matches(INSTALL).count();
        if total == 0 {
            continue;
        }
        let mut accounted = 0usize;
        for name in declared_function_names(source) {
            let body = function_body(source, &name);
            let hits = body.matches(INSTALL).count();
            if hits == 0 {
                continue;
            }
            accounted = accounted.saturating_add(hits);
            out.push(TtbrInstall {
                file: file.clone(),
                function: name.clone(),
                signature: function_signature(source, &name),
                body: body.to_string(),
            });
        }
        assert!(
            accounted >= total,
            "{file}: the census reached {accounted} of {total} TTBR0 install occurrences, so at \
             least one is in no censused function"
        );
    }
    out
}

/// The identifier handed to the asm block as the value being installed.
fn install_operand(body: &str) -> String {
    match body.split_once("in(reg)") {
        Some((_, rest)) => rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect(),
        None => String::new(),
    }
}

#[test]
fn every_ttbr0_install_settles_the_per_cpu_shadows() {
    let sources = rust_sources_below("kernel/src");
    let census = ttbr0_install_census(&sources);
    assert!(
        census.len() >= 5,
        "the TTBR0 install census reached only {} functions, so it is not covering the code it \
         claims to",
        census.len()
    );

    let mut unreconciled = Vec::new();
    for install in &census {
        if install.file == "kernel/src/arch_impl/aarch64/ttbr0.rs" {
            continue;
        }
        if install.body.contains("set_saved_process_cr3") {
            continue;
        }
        let operand = install_operand(&install.body);
        if !operand.is_empty() && traces_to_a_parameter(&install.signature, &install.body, &operand)
        {
            continue;
        }
        unreconciled.push(format!("{}::{}", install.file, install.function));
    }

    let escaped: Vec<&String> = unreconciled
        .iter()
        .filter(|entry| {
            !TIER_ONE_PROHIBITED
                .iter()
                .any(|tier_one| entry.starts_with(tier_one))
        })
        .collect();
    assert!(
        escaped.is_empty(),
        "these TTBR0 installs leave the per-CPU shadows naming another root: {escaped:?}"
    );

    // Print, rather than pin, what the Tier-1 rule is holding back: these are
    // real members of the same defect class that this branch was not allowed to
    // touch. If the list empties because someone repaired them, the test still
    // passes; if it empties because the census stopped reaching them, the
    // coverage floor above is what notices.
    if !unreconciled.is_empty() {
        eprintln!(
            "TTBR0 installs still unreconciled behind the Tier-1 rule: {unreconciled:?}"
        );
    }
}

#[test]
fn the_shadow_census_rejects_an_unreconciled_install() {
    let invented = "fn install_a_process_root() {\n\
        let ttbr0_value = page_table.level_4_frame().start_address().as_u64();\n\
        unsafe {\n\
            core::arch::asm!(\"msr ttbr0_el1, {}\", in(reg) ttbr0_value);\n\
        }\n\
    }\n";
    let sources = vec![(
        "kernel/src/arch_impl/aarch64/invented.rs".to_string(),
        invented.to_string(),
    )];
    let census = ttbr0_install_census(&sources);
    let site = census
        .iter()
        .find(|install| install.function == "install_a_process_root")
        .expect("the census must see a newly added install");
    assert!(
        !site.body.contains("set_saved_process_cr3"),
        "the perturbed site must be unreconciled"
    );
    assert!(
        !traces_to_a_parameter(&site.signature, &site.body, &install_operand(&site.body)),
        "a root read out of the process manager must not be classified as parameter-borne"
    );
}

#[test]
fn the_shadow_census_accepts_a_mechanism_primitive() {
    let signature = "    unsafe fn write_root(addr: u64) {";
    let body = "let aligned = addr & 0x0000_FFFF_FFFF_F000; \
                core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) aligned);";
    assert!(
        traces_to_a_parameter(signature, body, &install_operand(body)),
        "a primitive that installs what its caller handed it must not be asked to own the shadows"
    );
}

#[test]
fn the_tier_one_exemption_matches_the_project_rule() {
    let claude_md = repo_text("CLAUDE.md");
    let prohibited = claude_md
        .split("PROHIBITED CODE SECTIONS")
        .nth(1)
        .expect("CLAUDE.md must carry its prohibited-sections table");
    for path in TIER_ONE_PROHIBITED {
        assert!(
            prohibited.contains(path),
            "{path} is exempted as Tier-1, but CLAUDE.md does not list it as prohibited"
        );
    }
}

#[test]
fn the_ttbr0_discipline_publishes_both_shadows() {
    let ttbr0 = repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs");
    let adopt = function_body(&ttbr0, "adopt_process_ttbr0");
    assert!(
        adopt.contains("msr ttbr0_el1"),
        "the discipline helper must be the one that installs the register"
    );
    assert!(
        adopt.contains("set_saved_process_cr3(ttbr0_value)"),
        "adopting a process root must publish it as the root the return corridor restores"
    );
    assert!(
        adopt.contains("set_next_cr3(0)"),
        "adopting a process root must retire the pending switch the corridor would apply first"
    );
}

#[test]
fn init_reaches_userspace_on_the_root_the_shadows_name() {
    let main = repo_text("kernel/src/main_aarch64.rs");
    let launch = function_body(&main, "launch_init_from_elf");
    assert!(
        launch.contains("ttbr0::adopt_process_ttbr0(ttbr0_value)"),
        "init is the one thread that reaches EL0 without a scheduler dispatch, so its own \
         install has to reconcile the shadows"
    );
    assert!(
        !launch.contains("msr ttbr0_el1"),
        "a raw install here leaves the idle redirect kernel root armed in next_cr3"
    );
    let install = launch.find("adopt_process_ttbr0").unwrap();
    let eret = launch.rfind("return_to_userspace(entry_point").unwrap();
    assert!(
        install < eret,
        "the shadows must agree with the register before the ERET to EL0"
    );
}

#[test]
fn an_unreadable_identity_is_not_a_refusal() {
    let idle_sleep = repo_text("kernel/src/task/idle_sleep.rs");
    let predicate = function_body(&idle_sleep, "idle_identity_must_not_sleep");
    let none_arm = predicate
        .split("None =>")
        .nth(1)
        .expect("the lock-free predicate must still handle an unreadable identity");
    assert!(
        !none_arm.contains("refuse_idle_identity") && !none_arm.trim_start().starts_with("true"),
        "a contended try_lock says nothing about who is running: refusing on it turns one \
         momentary lock collision into a permanent verdict for the caller"
    );
    assert!(
        none_arm.contains("IDLE_IDENTITY_UNREADABLE"),
        "the one place this predicate is permissive has to stay measurable"
    );

    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let under_lock = function_body(&scheduler, "refuse_idle_block");
    assert!(
        under_lock.contains("current_thread == Some(")
            && under_lock.contains("idle_sleep::refuse_idle_identity"),
        "the authoritative refusal must still read the identity exactly, under the scheduler lock"
    );
    assert!(
        scheduler.matches("self.refuse_idle_block()").count() >= 6,
        "every blocking primitive that publishes the caller blocked state must keep calling it"
    );
}
