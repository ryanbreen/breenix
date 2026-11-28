//! E2.4's checks (docs/plans/e2-sleepable-contexts.md §6 and §8, "E2.4: block
//! I/O caller rule"): the DEV-24 regression and the boot handoff, through
//! AHCI admission's decision (`kernel/src/drivers/ahci/admission.rs`), which
//! `begin_port_io` calls.
//!
//! An ordinary sleepable caller already parks for an owned port, so a model of
//! one would be green before E2.4. The failing case was a context: PID 1's
//! image load from the boot continuation while `breenfs-commit` owned port 0
//! (DEV-24: `tid=Some(1) preempt_count=2 port_owner=21`). So the checks feed
//! the decision the contexts the boot code issues its loads from.

use crate::admission::{decide, Admission, Arch};
use crate::block::read_failure::AhciReadError;
use crate::park_core::ContextInputs;
use std::path::PathBuf;

/// The `breenfs-commit` worker that owned port 0 in DEV-24's serial.
const OWNER: u64 = 21;

/// DEV-24's reader: the boot continuation, CPU 0's idle thread, at
/// preemption depth 2.
fn dev24() -> ContextInputs {
    ContextInputs { dispatching: true, has_thread: true, idle: true, depth: 2, ..ContextInputs::default() }
}

/// The ARM64 boot continuation once the scheduler dispatches: CPU 0's idle
/// thread, holding the boot pin `kernel_main` takes after per-CPU init.
fn arm64_boot_continuation() -> ContextInputs {
    ContextInputs { dispatching: true, has_thread: true, idle: true, depth: 1, ..ContextInputs::default() }
}

/// A kernel thread at baseline 0 and depth 0: the `init-loader` (design §6).
fn kernel_thread() -> ContextInputs {
    ContextInputs { dispatching: true, has_thread: true, ..ContextInputs::default() }
}

// ------------------------------------------------- the boot code's contexts

fn main_aarch64() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../kernel/src/main_aarch64.rs");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
    code_only(&source)
}

/// `source` with `//` comments and string literals blanked, offsets kept.
fn code_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        out[i] = b' ';
                        i += 1;
                    }
                    if bytes[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).expect("blanking keeps ASCII boundaries")
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Each `fn` item with the byte range of its body.
fn functions(code: &str) -> Vec<(String, usize, usize)> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    for (at, _) in code.match_indices("fn ") {
        if at > 0 && is_ident(bytes[at - 1]) {
            continue;
        }
        let name: String = code[at + 3..].chars().take_while(|c| is_ident(*c as u8)).collect();
        let after = at + 3 + name.len();
        if name.is_empty() || !code[after..].trim_start().starts_with(['(', '<']) {
            continue;
        }
        let (Some(brace), semi) = (code[after..].find('{'), code[after..].find(';')) else { continue };
        if semi.is_some_and(|semi| semi < brace) {
            continue;
        }
        let open = after + brace;
        let mut depth = 0usize;
        for (offset, &byte) in bytes[open..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        found.push((name.clone(), open, open + offset));
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// The calls of `name(` in `code` that are not its definition, by offset.
fn calls_of(code: &str, name: &str) -> Vec<usize> {
    let bytes = code.as_bytes();
    code.match_indices(name)
        .filter(|(at, _)| {
            let before = *at == 0 || !is_ident(bytes[at - 1]);
            let defined = code[..*at].trim_end().ends_with("fn");
            before && !defined && code[at + name.len()..].trim_start().starts_with('(')
        })
        .map(|(at, _)| at)
        .collect()
}

/// The init-image loads `main_aarch64.rs` makes once the scheduler dispatches,
/// each with the function that makes it and the context it runs in. The first
/// dispatch follows `timer_interrupt::init()` in `kernel_main`: CPU 0's timer
/// runs from there, and the secondaries are released after it.
fn init_loads_after_first_dispatch() -> Vec<(String, ContextInputs)> {
    let code = main_aarch64();
    let fns = functions(&code);
    let (_, main_open, main_close) =
        fns.iter().find(|(name, ..)| name == "kernel_main").cloned().expect("ARM64 kernel_main exists");
    let dispatch = main_open
        + code[main_open..main_close].find("timer_interrupt::init()").expect("kernel_main starts the timer");
    let mut loads = Vec::new();
    for entry in ["read_init_from_ext2", "run_userspace_from_disk"] {
        for at in calls_of(&code, entry) {
            let (function, ..) = fns
                .iter()
                .filter(|(_, open, close)| *open <= at && at <= *close)
                .min_by_key(|(_, open, close)| close - open)
                .expect("each load is inside a function");
            let context = match function.as_str() {
                "kernel_main" if at < dispatch => continue,
                "kernel_main" => arm64_boot_continuation(),
                "run_init_loader" => kernel_thread(),
                other => panic!("{entry}() in {other}: name the context that function runs in"),
            };
            loads.push((format!("{entry}() in {function}"), context));
        }
    }
    loads
}

// --------------------------------------------------------------- the checks

/// The seam reaches the old path: the recorded DEV-24 context, against an
/// owned port, gets the old answer (`CannotPark` on x86, the `yield` poll on
/// ARM64).
#[test]
fn the_dev24_context_gets_the_old_answer_through_the_seam() {
    assert_eq!(decide(dev24(), OWNER, Arch::X86_64), Admission::Refuse(AhciReadError::CannotPark));
    assert_eq!(decide(dev24(), OWNER, Arch::Aarch64), Admission::Poll);
}

/// The boot handoff: each init-image load the ARM64 boot makes once the
/// scheduler dispatches parks for a port another thread owns, and gets the
/// port when that owner releases it. Before E2.4 the post-SMP fallbacks run on
/// the boot continuation, CPU 0's idle context, which may not park, and ARM64
/// polls there.
#[test]
fn init_loads_after_the_first_dispatch_park_for_an_owned_port() {
    let loads = init_loads_after_first_dispatch();
    assert!(!loads.is_empty(), "no init-image load after the first dispatch: the late fallback is gone");
    let wrong: Vec<String> = loads
        .iter()
        .filter_map(|(site, context)| {
            let answer = decide(*context, OWNER, Arch::Aarch64);
            (answer != Admission::Park).then(|| format!("{site}: {answer:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "init-image loads that do not park for an owned port:\n  {}", wrong.join("\n  "));
    for (site, context) in &loads {
        assert_eq!(decide(*context, 0, Arch::Aarch64), Admission::Acquire, "{site}: a released port");
    }
}

/// Under the new rule the DEV-24 context is refused on both architectures:
/// not polled, not parked, and not the old `CannotPark` that BreenFS turned
/// into EAGAIN.
#[test]
fn the_dev24_context_is_refused_not_polled() {
    for arch in [Arch::X86_64, Arch::Aarch64] {
        let answer = decide(dev24(), OWNER, arch);
        assert!(
            matches!(answer, Admission::Refuse(error) if error != AhciReadError::CannotPark),
            "{arch:?}: the DEV-24 context got {answer:?}"
        );
    }
}
