use std::{env, fs, path::PathBuf};

fn main() {
    let kernel = PathBuf::from("../../kernel/src");
    let context_switch = kernel.join("arch_impl/aarch64/context_switch.rs");
    println!("cargo:rerun-if-changed={}", context_switch.display());
    let source = fs::read_to_string(context_switch).unwrap();
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
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"coreproof_mut_cpu_identity\"))");
    fs::write(
        out.join("inline_entry.rs"),
        format!("{retry}\nfn inline_entry() -> Option<usize> {{{entry}\n    let _ = saved_daif;\n    assert_eq!(cpu_id, (&mut guard).cpu());\n    Some(cpu_id)\n}}").replace("            return;", "            return None;"),
    ).unwrap();
}
