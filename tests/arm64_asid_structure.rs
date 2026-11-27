//! Structural checks for ARM64 per-address-space ASIDs (audit R19, DEV-29).
//!
//! The allocator's own behaviour is checked by the host model in
//! `tools/asid-host-tests`. What that model cannot see is whether the kernel
//! uses it: whether an address-space switch still broadcasts a full TLB
//! invalidation, whether user pages are non-global (an ASID only separates
//! non-global entries), whether the kernel root's TTBR0 identity map is
//! non-global too (a global entry there would match a user VA under any
//! ASID), and whether 16-bit ASIDs are switched on when the CPU has them.

use std::fs;
use std::path::PathBuf;

fn repo_text(relative: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(relative)).unwrap_or_else(|_| panic!("read {relative}"))
}

/// The source with `//` line comments, `///` doc comments and `/* */` block
/// comments removed, so a word in prose is not mistaken for code.
fn code_only(source: &str) -> String {
    let mut out = String::new();
    let mut rest = source;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..].find("*/").map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    out.lines()
        .map(|line| line.find("//").map_or(line, |cut| &line[..cut]))
        .collect::<Vec<_>>()
        .join("\n")
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}(");
    let start = source.find(&signature).unwrap_or_else(|| panic!("find fn {name}"));
    let open = start + source[start..].find('{').unwrap_or_else(|| panic!("open brace of {name}"));
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
    panic!("close brace of {name}")
}

fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
    let start = source.find(from).unwrap_or_else(|| panic!("find {from:?}"));
    let end = start + source[start..].find(to).unwrap_or_else(|| panic!("find {to:?} after {from:?}"));
    &source[start..end]
}

/// An address-space switch installs a root tagged with that space's ASID and
/// needs no TLB invalidation. The 4 ttbr0.rs install functions below,
/// `switch_ttbr0_if_needed` and the exception-return corridor (6 places) must
/// therefore issue no `tlbi`: a
/// broadcast there costs each CPU its TLB on each switch.
#[test]
fn address_space_switches_issue_no_tlb_invalidation() {
    let ttbr0 = code_only(&repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs"));
    let context_switch = code_only(&repo_text("kernel/src/arch_impl/aarch64/context_switch.rs"));
    let corridor = code_only(&repo_text("kernel/src/arch_impl/aarch64/syscall_entry.S"));
    let mut offenders = Vec::new();
    for name in ["adopt_process_ttbr0", "restore_process_ttbr0", "switch_ttbr0_to_kernel", "install_ttbr0"] {
        if function_body(&ttbr0, name).contains("tlbi") {
            offenders.push(format!("ttbr0.rs::{name}"));
        }
    }
    if function_body(&context_switch, "switch_ttbr0_if_needed").contains("tlbi") {
        offenders.push("context_switch.rs::switch_ttbr0_if_needed".into());
    }
    if between(&corridor, ".Lsvc_elr_ok:", ".Lafter_ttbr_check:").contains("tlbi") {
        offenders.push("syscall_entry.S return corridor".into());
    }
    assert!(offenders.is_empty(), "switch paths still invalidate the TLB: {offenders:?}");

    // The one invalidation an ASID scheme needs on a switch is the local one
    // after a generation rollover, and it is the allocator's to ask for.
    let bind = function_body(&ttbr0, "bind_process_root");
    assert!(bind.contains("flush_local"), "bind must honour the allocator's local-flush request");
    assert!(
        ttbr0.contains("tlbi vmalle1\"") || ttbr0.contains("\"tlbi vmalle1\","),
        "the rollover flush is a local `tlbi vmalle1`, not the broadcast `vmalle1is`"
    );
}

/// An ASID only separates non-global entries: user leaves must set nG, and
/// reading a descriptor back must keep it (copy-on-write and mprotect rewrite
/// leaves from their decoded flags).
#[test]
fn user_leaves_are_non_global() {
    let stub = code_only(&repo_text("kernel/src/memory/arch_stub.rs"));
    assert!(stub.contains("const DESC_NG: u64 = 1 << 11;"), "arch_stub must name the nG bit");
    let encode = function_body(&stub, "to_arm64_descriptor");
    assert!(
        encode.contains("Self::GLOBAL") && encode.contains("DESC_NG"),
        "to_arm64_descriptor must set nG unless the mapping asks to be GLOBAL"
    );
    let decode = function_body(&stub, "flags");
    assert!(decode.contains("DESC_NG"), "PageTableEntry::flags must decode nG back to GLOBAL");
}

/// The kernel root stays installed in TTBR0 while a CPU idles or runs kernel
/// threads, and the switch back to a user root no longer invalidates.
/// Its identity-map blocks are tagged with the kernel ASID (nG set), so they
/// cannot satisfy a user access under any user ASID. Both boot paths build it:
/// boot.S on QEMU and the UEFI loader on Parallels and VMware.
#[test]
fn kernel_ttbr0_identity_blocks_are_non_global() {
    let boot = code_only(&repo_text("kernel/src/arch_impl/aarch64/boot.S"));
    assert!(boot.contains(".equ DESC_NG,"), "boot.S must define the nG bit");
    let ttbr0_setup = between(&boot, "setup_mmu:", "ttbr1_l0\n");
    assert!(
        ttbr0_setup.contains("DESC_NG"),
        "boot.S's TTBR0 identity blocks must carry nG"
    );
    let fill = between(&boot, "fill_l2_ram:", "ret");
    assert!(fill.contains("x2"), "fill_l2_ram takes the extra (nG) attribute in x2");

    let loader = code_only(&repo_text("parallels-loader/src/page_tables.rs"));
    assert!(loader.contains("pub const NG: u64 = 1 << 11;"), "the loader must name the nG bit");
    let raw: Vec<&str> = loader
        .lines()
        .filter(|line| line.contains("write_entry(ttbr0_") && !line.contains("TABLE_DESC"))
        .collect();
    assert!(raw.is_empty(), "TTBR0 blocks written without nG: {raw:?}");
    let identity = function_body(&loader, "write_identity_block");
    assert!(identity.contains("attr::NG"), "write_identity_block must set nG");
}

/// ID_AA64MMFR0_EL1.ASIDBits says whether the CPU has 16-bit ASIDs; using them
/// needs TCR_EL1.AS, and the ASID must come from TTBR0 (TCR_EL1.A1 clear).
#[test]
fn asid_width_follows_the_id_register() {
    let ttbr0 = code_only(&repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs"));
    let init = function_body(&ttbr0, "init_asids");
    assert!(init.contains("id_aa64mmfr0_el1"), "init_asids must read ASIDBits");
    assert!(init.contains("TCR_AS"), "16-bit ASIDs need TCR_EL1.AS");
    assert!(init.contains("TCR_A1"), "init_asids must keep the ASID in TTBR0");
    assert!(ttbr0.contains("const TCR_AS: u64 = 1 << 36;"));
    assert!(ttbr0.contains("const TCR_A1: u64 = 1 << 22;"));
    let main = code_only(&repo_text("kernel/src/main_aarch64.rs"));
    let asids = main.find("ttbr0::init_asids()").expect("kernel_main must initialise ASIDs");
    let smp = main.find("smp::set_smp_ttbrs()").expect("kernel_main hands secondaries their MMU state");
    assert!(asids < smp, "secondary CPUs copy CPU 0's TCR, so ASIDs are set up first");
}

/// Unmap invalidation targets the one space's ASID when it has a current one.
#[test]
fn user_invalidation_targets_the_space_asid() {
    let tlb = code_only(&repo_text("kernel/src/memory/tlb.rs"));
    for name in ["flush_user_page", "flush_user_all", "flush_user_tables", "flush_user_pages"] {
        let body = function_body(&tlb, name);
        assert!(
            body.contains("user_asid") || body.contains("flush_user_page(") || body.contains("flush_user_all("),
            "{name} must look up the space's ASID"
        );
        assert!(!body.contains("USER_ASID_TTBR0"), "{name} still assumes one shared user ASID");
    }
}
