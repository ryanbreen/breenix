use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn source(path: &str) -> String {
    fs::read_to_string(root().join(path)).expect("source file must be readable")
}

fn source_files_with_extension(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("source directory must be readable") {
        let path = entry.expect("source entry must be readable").path();
        if path.is_dir() {
            source_files_with_extension(&path, extension, out);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            out.push(path);
        }
    }
}

fn slice_until<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let start_offset = text.find(start).expect("region start must exist");
    let rest = &text[start_offset..];
    let end_offset = rest.find(end).expect("region end must exist");
    &rest[..end_offset]
}

fn function_source<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text.find(signature).expect("function signature must exist");
    let body_start = text[start..]
        .find('{')
        .map(|offset| start + offset)
        .expect("function body must exist");
    let mut depth = 0usize;
    for (offset, byte) in text.as_bytes()[body_start..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[start..=body_start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("function body must terminate");
}

fn fnv1a64(text: &str) -> u64 {
    text.as_bytes().iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[test]
fn exception_return_tails_do_not_drain_or_reclaim() {
    let context = source("kernel/src/arch_impl/aarch64/context_switch.rs");
    assert!(!context.contains("drain_deferred_fault_sigsegv_exits"));
    for signature in [
        "pub extern \"C\" fn check_need_resched_and_switch_arm64(",
        "pub fn schedule_from_kernel()",
    ] {
        let body = function_source(&context, signature);
        for forbidden in [
            "reclaim_pass(",
            "reclaim_one(",
            "cleanup_for_exec(",
            "take_deferred_fault_sigsegv_exit(",
        ] {
            assert!(
                !body.contains(forbidden),
                "ERET tail {signature} contains {forbidden}"
            );
        }
    }
}

#[test]
fn assembly_publishes_installed_root_before_clearing_pending_lease() {
    let assembly = source("kernel/src/arch_impl/aarch64/syscall_entry.S");
    for sequence in [
        "msr ttbr0_el1, x1\n    isb\n    str x1, [x0, #80]           /* saved_process_cr3 = installed root */\n    dmb ishst                    /* release saved before clearing next */\n    str xzr, [x0, #64]          /* clear next_cr3 last */",
        "msr ttbr0_el1, x10\n    isb\n    str x10, [x9, #80]          /* saved_process_cr3 = installed root */\n    dmb ishst                    /* release saved before clearing next */\n    str xzr, [x9, #64]          /* clear next_cr3 last */",
    ] {
        assert!(assembly.contains(sequence));
    }
    assert_eq!(assembly.matches("release saved before clearing next").count(), 2);
}

#[test]
fn no_new_rust_ttbr0_writer_escapes_the_reviewed_set() {
    let mut files = Vec::new();
    source_files_with_extension(&root().join("kernel/src"), "rs", &mut files);
    let mut writers: Vec<_> = files
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("Rust source must be readable")
                .contains("msr ttbr0_el1")
        })
        .map(|path| {
            path.strip_prefix(root())
                .expect("source must be below repository root")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    writers.sort();
    assert_eq!(
        writers,
        [
            "kernel/src/arch_impl/aarch64/paging.rs",
            "kernel/src/arch_impl/aarch64/ttbr0.rs",
            "kernel/src/main_aarch64.rs",
            "kernel/src/memory/arch_stub.rs",
            "kernel/src/syscall/graphics.rs",
            "kernel/src/syscall/handlers.rs",
            "kernel/src/syscall/time.rs",
            "kernel/src/syscall/wait.rs",
        ]
    );
}

#[test]
fn reclaim_capability_has_only_the_reviewed_mint_sites() {
    let mut files = Vec::new();
    source_files_with_extension(&root().join("kernel/src"), "rs", &mut files);
    let matches: usize = files
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .expect("Rust source must be readable")
                .matches("ReclaimContext::assert_preemptible()")
                .count()
        })
        .sum();
    assert_eq!(matches, 4, "unexpected ReclaimContext capability mint");
    assert_eq!(
        source("kernel/src/arch_impl/aarch64/syscall_entry.rs")
            .matches("ReclaimContext::assert_preemptible()")
            .count(),
        1
    );
    assert_eq!(
        source("kernel/src/task/reclaim.rs")
            .matches("ReclaimContext::assert_preemptible()")
            .count(),
        3
    );

    let process = source("kernel/src/process/process.rs");
    let memory = source("kernel/src/memory/process_memory.rs");
    let reclaim = source("kernel/src/task/reclaim.rs");
    assert!(!process.contains("cleanup_cow_frames"));
    assert!(!process.contains("cleanup_cow_page_table"));
    assert!(
        memory.contains("cleanup_for_exec(self, _context: &crate::task::reclaim::ReclaimContext)")
    );
    assert!(reclaim.contains("fn release_stack(&mut self, _context: &ReclaimContext)"));
}

#[test]
fn unconstrained_ttbr0_teardown_helpers_stay_deleted() {
    let ttbr0 = source("kernel/src/arch_impl/aarch64/ttbr0.rs");
    for removed in [
        "switch_ttbr0_to_kernel",
        "quiesce_ttbr0_for_exit",
        "current_cpu_retains_ttbr0_root",
    ] {
        assert!(
            !ttbr0.contains(removed),
            "unconstrained helper returned: {removed}"
        );
    }
}

#[test]
fn every_new_teardown_counter_has_an_in_tree_reader() {
    let reclaim = source("kernel/src/task/reclaim.rs");
    for counter in [
        "GRAVES_QUEUED",
        "GRAVES_RECLAIMED",
        "GRAVES_BLOCKED",
        "FAULT_EXIT_INTENT_DROPPED",
        "FRAME_DECREF_UNDERFLOW",
        "FRAME_DECREF_UNTRACKED",
    ] {
        assert!(
            reclaim.contains(counter),
            "counter {counter} must be read by dump_reclaim_state"
        );
    }
}

#[test]
fn frozen_regions_match_the_reviewed_gold_masters() {
    let context = source("kernel/src/arch_impl/aarch64/context_switch.rs");
    let gic = source("kernel/src/arch_impl/aarch64/gic.rs");
    let timer = source("kernel/src/arch_impl/aarch64/timer_interrupt.rs");

    let regions = [
        (
            "EL0 dispatch banner/guard",
            slice_until(
                &context,
                "        // 🔒 GOLD-MASTER REGION — DO NOT ADD A CPU0-SPECIFIC EL0 DISPATCH GUARD",
                "\n\n        if !has_started {",
            ),
            0xeb434b54929bf2bf,
        ),
        (
            "idle_loop_arm64 body",
            function_source(&context, "pub extern \"C\" fn idle_loop_arm64()"),
            0x7ff37a17a3a7c666,
        ),
        (
            "aarch64_enter_exception_frame ISB placement",
            slice_until(
                &context,
                "aarch64_enter_exception_frame:\n",
                "\n\"#\n);",
            ),
            0xb7bc078a61fed816,
        ),
        (
            "GIC SGI enable block",
            slice_until(
                &gic,
                "    // 🔒 GOLD-MASTER REGION — SGI admission enable",
                "\n}\n\n/// Initialize GICv3 CPU Interface",
            ),
            0xace96ea222a5c040,
        ),
        (
            "timer arm-at-top block",
            slice_until(
                &timer,
                "    // 🔒 GOLD-MASTER REGION — arm_timer at TOP of handler",
                "\n\n    // Snapshot CNTV_CTL_EL0 for this CPU",
            ),
            0x1d9e56fabecb5d2b,
        ),
        (
            "CPU0 regression alarm",
            slice_until(
                &timer,
                "    // 🔒 GOLD-MASTER REGION — CPU0 divergence regression alarm",
                "\n\n    // Mask the timer interrupt at the source",
            ),
            0x7e8455c0595c6290,
        ),
    ];

    for (name, region, expected) in regions {
        assert_eq!(fnv1a64(region), expected, "frozen region changed: {name}");
    }
}

#[test]
fn lease_publication_is_confined_to_the_reviewed_assembly_file() {
    assert_eq!(
        fnv1a64(&source("kernel/src/arch_impl/aarch64/boot.S")),
        0x3402d3cdd7e3c9d6,
        "an assembly file outside the reviewed syscall-entry boundary changed"
    );
    let mut files = Vec::new();
    source_files_with_extension(&root().join("kernel/src"), "S", &mut files);
    let mut publishers: Vec<_> = files
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("assembly source must be readable")
                .contains("saved_process_cr3 = installed root")
        })
        .map(|path| {
            path.strip_prefix(root())
                .expect("assembly source must be below repository root")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    publishers.sort();
    assert_eq!(
        publishers,
        ["kernel/src/arch_impl/aarch64/syscall_entry.S"]
    );
}
