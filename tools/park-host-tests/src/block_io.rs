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

use crate::admission::{decide, Admission};
use crate::block::read_failure::AhciReadError;
use crate::park_core::{ContextInputs, IoContext};
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

/// A kernel thread holding one `park::Braked` level, its depth and baseline
/// raised together: the `init-loader`, which holds it as the `test-loader`
/// and x86's `boot_storage` do (design §6, as built).
fn braked_kernel_thread() -> ContextInputs {
    ContextInputs { dispatching: true, has_thread: true, depth: 1, baseline: 1, ..ContextInputs::default() }
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
                "run_init_loader" => braked_kernel_thread(),
                other => panic!("{entry}() in {other}: name the context that function runs in"),
            };
            loads.push((format!("{entry}() in {function}"), context));
        }
    }
    loads
}

// --------------------------------------------------------------- the checks

/// The boot handoff: each init-image load the ARM64 boot makes once the
/// scheduler dispatches parks for a port another thread owns, and gets the
/// port when that owner releases it. Before E2.4 the post-SMP fallbacks ran on
/// the boot continuation, CPU 0's idle context, which may not park, and ARM64
/// polled there.
#[test]
fn init_loads_after_the_first_dispatch_park_for_an_owned_port() {
    let loads = init_loads_after_first_dispatch();
    assert!(!loads.is_empty(), "no init-image load after the first dispatch: the late fallback is gone");
    let wrong: Vec<String> = loads
        .iter()
        .filter_map(|(site, context)| {
            // The token is revoked at the first dispatch.
            let answer = decide(*context, false, OWNER);
            (answer != Admission::Park).then(|| format!("{site}: {answer:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "init-image loads that do not park for an owned port:\n  {}", wrong.join("\n  "));
    for (site, context) in &loads {
        assert_eq!(decide(*context, false, 0), Admission::Acquire(IoContext::Sleepable), "{site}: a released port");
    }
}

/// Under the new rule the DEV-24 context is refused on both architectures:
/// not polled, not parked, and not the old `CannotPark` that BreenFS turned
/// into EAGAIN.
#[test]
fn the_dev24_context_is_refused_not_polled() {
    assert_eq!(decide(dev24(), false, OWNER), Admission::Refuse(AhciReadError::NotSleepable));
}

/// An atomic caller is refused whatever the port's state, so it gets no data:
/// a free port is not taken either. Through the production rule
/// (`park_core::io_context`) the refusal writes a receipt and returns
/// `NotSleepable`; it never polls.
#[test]
fn an_atomic_caller_gets_not_sleepable_a_receipt_and_no_data() {
    use crate::model::{Event, Model};
    use crate::park_core::{io_context, EarlyBootIo, ParkContext, ParkError};
    let atomic = [
        ("hard IRQ", ContextInputs { dispatching: true, has_thread: true, hardirq: true, ..ContextInputs::default() }),
        ("a held spin lock", ContextInputs { dispatching: true, has_thread: true, depth: 1, ..ContextInputs::default() }),
        ("the idle continuation", arm64_boot_continuation()),
        ("interrupts masked", ContextInputs { dispatching: true, has_thread: true, irqs_masked: true, ..ContextInputs::default() }),
    ];
    let token = EarlyBootIo::new();
    assert!(token.mint() && token.revoke());
    for (name, context) in atomic {
        for owner in [0, OWNER] {
            assert_eq!(decide(context, false, owner), Admission::Refuse(AhciReadError::NotSleepable), "{name}, owner {owner}");
        }
        let model = Model::new(0, context.baseline);
        model.inputs.set(Some(context));
        let refused = crate::park_core::classify(context);
        assert!(matches!(refused, ParkContext::Atomic(_)), "{name}: {refused:?}");
        assert_eq!(io_context(&model, 0x24, &token), Err(ParkError::NotSleepable(refused)), "{name}");
        assert_eq!(model.events(), vec![Event::Receipt(refused)], "{name}: one receipt");
    }
}

/// The early-boot token: minted once, it lets the boot thread poll; revoked
/// at the first dispatch, early boot's polling is refused, and it cannot be
/// minted again. While it is held the boot thread owns the controller, so
/// polled I/O against a port another thread owns is refused, not waited for.
#[test]
fn the_early_boot_token_is_refused_after_revocation_and_against_an_owned_port() {
    use crate::park_core::{io_rule, EarlyBootIo, ParkContext};
    let early = ContextInputs::default();
    assert_eq!(crate::park_core::classify(early), ParkContext::EarlyBoot);
    let token = EarlyBootIo::new();
    assert_eq!(io_rule(ParkContext::EarlyBoot, token.held()), Err(ParkContext::EarlyBoot), "before the mint");
    assert!(token.mint());
    assert!(!token.mint(), "minted twice");
    assert_eq!(io_rule(ParkContext::EarlyBoot, token.held()), Ok(IoContext::EarlyBoot));
    assert_eq!(decide(early, token.held(), 0), Admission::Acquire(IoContext::EarlyBoot));
    assert_eq!(decide(early, token.held(), OWNER), Admission::Refuse(AhciReadError::EarlyBootPortOwned));
    assert!(token.revoke());
    assert!(token.revoked() && !token.held());
    assert!(!token.mint(), "minted again after revocation");
    assert!(!token.revoke(), "revoked twice");
    assert_eq!(io_rule(ParkContext::EarlyBoot, token.held()), Err(ParkContext::EarlyBoot));
    assert_eq!(decide(early, token.held(), 0), Admission::Refuse(AhciReadError::NotSleepable));
}

/// The testing loader holds its CPU between binaries. One bare level at
/// baseline 0 is `LocksHeld(1)`, which E2.4 refuses; the same level held as
/// `park::Braked` raises the baseline with it, so its reads are a sleepable
/// context and park for an owned port.
#[test]
fn a_braked_level_is_sleepable_and_a_bare_level_is_not() {
    use crate::park_core::{AtomicReason, ParkContext};
    let bare = ContextInputs { dispatching: true, has_thread: true, depth: 1, ..ContextInputs::default() };
    assert_eq!(crate::park_core::classify(bare), ParkContext::Atomic(AtomicReason::LocksHeld(1)));
    assert_eq!(decide(bare, false, OWNER), Admission::Refuse(AhciReadError::NotSleepable));
    assert_eq!(crate::park_core::classify(braked_kernel_thread()), ParkContext::Sleepable);
    assert_eq!(decide(braked_kernel_thread(), false, OWNER), Admission::Park);
}

/// The revocation assertion (design §6, "Completion readiness before
/// revocation"): revoking the token while an attached device's completion
/// source is unarmed names that device, and its sleeping requests are refused
/// and counted, not polled, until the source is armed.
#[test]
fn revocation_names_an_unarmed_device_and_its_requests_are_refused() {
    use crate::park_core::{revoke, CompletionSources, EarlyBootIo};
    const ARMED: u64 = 3 << 32;
    const UNARMED: u64 = 1 << 32;
    let sources = CompletionSources::<4>::new();
    assert!(sources.attach(ARMED, true));
    assert!(sources.attach(UNARMED, false));
    let token = EarlyBootIo::new();
    assert!(token.mint());
    let mut unready = [0u64; 4];
    assert_eq!(revoke(&token, &sources, &mut unready), Some(1));
    assert_eq!(unready[0], UNARMED);
    assert!(token.revoked());
    assert!(sources.ready(ARMED));
    assert!(!sources.ready(UNARMED));
    assert!(!sources.ready(UNARMED));
    assert_eq!(sources.refusals(UNARMED), 2);
    assert!(!sources.ready(5 << 32), "a device that never attached is not ready");
    sources.arm(UNARMED);
    assert!(sources.ready(UNARMED));
    assert_eq!(revoke(&token, &sources, &mut unready), None, "the token is revoked once");
    let full = CompletionSources::<1>::new();
    assert!(full.attach(ARMED, true));
    assert!(!full.attach(UNARMED, true), "a full table refuses the device, which is then never ready");
    assert!(!full.ready(UNARMED));
}

/// DEV-48(3), with secondaries that come online out of order (#3298 review):
/// a CPU's AHCI completion poller starts once the scheduler's count covers it,
/// in 5 of 5 arrival orders below, and not for a CPU the count does not yet
/// cover (`spawn_on_cpu` refuses one). The model replays
/// `secondary_cpu_entry_rust`: mark the CPU online, count it, then start what
/// `pollers_to_start` names.
#[test]
fn every_online_cpu_gets_a_poller_whatever_order_secondaries_arrive_in() {
    use crate::admission::pollers_to_start;
    for order in [[1, 2, 3], [1, 3, 2], [3, 2, 1], [2, 3, 1], [3, 1, 2]] {
        // CPU 0 starts its poller at the first dispatch, before any release.
        let (mut online, mut counted) = (1u64, 1usize);
        let mut spawned = pollers_to_start(0, counted, online, 0);
        for cpu in order {
            online |= 1 << cpu;
            counted += 1;
            let start = pollers_to_start(cpu, counted, online, spawned);
            assert_eq!(start & !((1u64 << counted) - 1), 0, "{order:?}: started a CPU the count does not cover");
            assert_eq!(start & spawned, 0, "{order:?}: started a poller twice");
            spawned |= start;
        }
        assert_eq!(spawned, 0b1111, "{order:?}: an online CPU has no completion poller");
    }
    // A CPU past a hole the count does not close stays without one.
    assert_eq!(pollers_to_start(3, 3, 0b1011, 0b0011), 0);
}

/// The #3298 review: the kernel's completion-source table holds the 34
/// sources the block drivers can attach: AHCI's controller, virtio-blk-pci's
/// device and 32 VirtIO MMIO block devices. A device whose attach finds the
/// table full is not ready, so its post-dispatch requests are refused.
#[test]
fn the_completion_source_table_holds_every_attachable_block_device() {
    use crate::park_core::{
        BlockSource, CompletionSources, BLOCK_COMPLETION_SOURCES, VIRTIO_BLK_MMIO_COMPLETION_SOURCES,
    };
    let sources = CompletionSources::<BLOCK_COMPLETION_SOURCES>::new();
    let mut keys = vec![BlockSource::Ahci.key(0), BlockSource::VirtioBlkPci.key(0)];
    keys.extend((0..VIRTIO_BLK_MMIO_COMPLETION_SOURCES as u32).map(|index| BlockSource::VirtioBlkMmio.key(index)));
    for &key in &keys {
        assert!(sources.attach(key, true), "device {key:#x} found the table full");
    }
    for &key in &keys {
        assert!(sources.ready(key), "device {key:#x} is armed but not ready");
    }
}
