use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn read(path: &Path) -> String {
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(path).unwrap()
}

fn inline_entry(source: &str, out: &Path) {
    let (_, retry) = source.split_once("fn lock_dispatch_from_task()").unwrap();
    let (retry, _) = retry.split_once("\n}\n").unwrap();
    let (before_asm, asm) = retry.split_once("        unsafe {").unwrap();
    let (_, after_asm) = asm.split_once("\n        }").unwrap();
    let retry = format!(
        "fn lock_dispatch_from_task(){before_asm}        retry_with_irqs_enabled();{after_asm}\n}}"
    );
    let (_, entry) = source
        .split_once("pub fn schedule_from_kernel() {")
        .unwrap();
    let (entry, _) = entry.split_once("    let sched = &mut guard;").unwrap();
    // The unavailable-scheduler exit is outside the case under test. The
    // fixture runs the production entry through acquisition, then observes
    // which CPU would be used for the dispatch transaction.
    let entry = entry.replace("super::irq_mask_account::", "irq_mask_account::").replace(
        "unsafe {\n                core::arch::asm!(\"msr daifclr, #3; isb\", options(nomem, nostack));\n            }",
        "restore_irqs();",
    );
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"coreproof_mut_cpu_identity\"))");
    fs::write(
        out.join("inline_entry.rs"),
        format!("{retry}\nfn inline_entry() -> Option<usize> {{{entry}\n    let _ = saved_daif;\n    assert_eq!(cpu_id, (&mut guard).cpu());\n    Some(cpu_id)\n}}").replace("            return;", "            return None;"),
    ).unwrap();
}

// Compile the production operations verbatim with the ARM64 cfg selected.
// The fixture supplies the scheduler storage, CPU identity and device edges.
fn item<'a>(source: &'a str, signature: &str, end: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing {signature}"));
    let end = source[start..].find(end).unwrap() + end.len();
    &source[start..start + end]
}

fn queue_operations(kernel: &Path, out: &Path) {
    let rq = read(&kernel.join("task/scheduler/rq.rs"));
    let arm = read(&kernel.join("task/scheduler/rq/arm64.rs"));
    let scheduler = read(&kernel.join("task/scheduler.rs"));
    let mut code = String::new();
    for signature in ["const RQ_HELD:", "const RQ_PEER_WORK:"] {
        code.push_str(item(&rq, signature, ";"));
        code.push('\n');
    }
    let (_, liveness) = rq.split_once("liveness\n").unwrap();
    let (timer_publication, _) = liveness.split_once("/// Claim a due deadline of a CPU other").unwrap();
    code.push_str(timer_publication);
    for signature in [
        "pub(super) struct RqLock {",
        "impl RqLock {",
        "pub(super) fn try_lock_all(",
        "pub(super) fn refresh(",
        "pub(super) struct Held {",
        "impl Drop for Held {",
        "fn kick_owed(",
        "pub(super) fn try_dispatch(",
        "pub(super) fn try_remote(",
        "fn try_steal_lock(",
        "fn is_peer_candidate(",
        "fn has_peer_work(",
        "fn send_reschedule(",
        "enum Target {",
        "fn wake_target(",
        "fn place_woken(",
        "fn kick(",
        "fn note_timer_pop(",
        "fn timer_entry_live(",
        "fn expire_heap(",
        "pub(super) fn expire_timers(",
        "pub(super) enum Wake {",
        "pub(super) enum Woken {",
        "pub(super) fn wake(",
        "pub(super) fn hold(",
        "pub(super) fn release_held_place(",
        "fn apply_wake(",
        "fn place_held(",
        "pub(super) fn apply_isr_wakeups(",
        "fn steal_from(",
    ] {
        code.push_str(item(&rq, signature, "\n}"));
        code.push('\n');
    }
    for signature in [
        "fn post_resched_arm64(",
        "fn request_resched_arm64(",
        "fn publish_wake(",
        "fn kick_wake_inbox(",
    ] {
        code.push_str(item(&scheduler, signature, "\n}"));
        code.push('\n');
    }
    code.push_str("impl Held {\n");
    for signature in [
        "    fn take(",
        "    pub(super) fn queue(",
        "    pub(super) fn state(",
        "    fn state_ref(",
        "    pub(super) fn heap(",
        "    fn publish(",
        "    pub(super) fn enqueue(",
    ] {
        code.push_str(item(&rq, signature, "\n    }"));
        code.push('\n');
    }
    code.push_str("}\n#[repr(transparent)]\n");
    code.push_str(item(&arm, "pub(crate) struct PivotToken(", ";"));
    code.push_str("\n");
    code.push_str(item(&arm, "pub(crate) struct Dispatch(", ";"));
    code.push_str("\nimpl Dispatch {\n");
    let (_, dispatch_methods) = arm.split_once("impl Dispatch {").unwrap();
    for signature in [
        "    pub(crate) fn try_local(",
        "    pub(crate) fn has_ready_work(",
        "    pub(crate) fn cpu(",
        "    pub(crate) fn into_pivot(",
        "    pub(crate) unsafe fn from_pivot(",
    ] {
        code.push_str(item(dispatch_methods, signature, "\n    }"));
        code.push('\n');
    }
    code.push_str("}\n");
    fs::write(
        out.join("queue_operations.rs"),
        code.replace("#[cfg(target_arch = \"aarch64\")]", "")
            .replace("#[cfg(target_arch = \"x86_64\")]", "#[cfg(any())]")
            .replace("super::post_resched_arm64", "post_resched_arm64")
            .replace("super::request_resched_arm64", "request_resched_arm64"),
    )
    .unwrap();
}

fn main() {
    let kernel = PathBuf::from("../../kernel/src");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = read(&kernel.join("arch_impl/aarch64/context_switch.rs"));
    inline_entry(&source, &out);
    queue_operations(&kernel, &out);
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"boot_tests\", \"ec0_fault_inject\"))");
}
