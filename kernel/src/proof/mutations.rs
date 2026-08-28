//! The planted-defect register.
//!
//! The harness's validation set is six defects this campaign already found and
//! fixed, re-introduced one at a time behind a cargo feature and kept in tree
//! forever as the harness's own regression suite. A harness that has never been
//! shown to re-find a known bug is an assertion, not an instrument.
//!
//! The re-introductions themselves live at the REAL sites — you cannot restore a
//! defect from a separate file — so this module is the register rather than the
//! code: it names, for each mutation, the issue it re-introduces, the PR that
//! fixed it, the file it perturbs, and the predicate expected to fire. That last
//! field is what makes a validation adjudicable: a mutation whose expected
//! outcome is unrecorded can only be argued about after the fact.
//!
//! `tests/coreproof_mutation_register_structure.rs` keeps three descriptions of
//! this set — the manifest's `[features]`, this register, and the `#[cfg]`
//! attributes at the sites — equal in both directions, as a census over the
//! `coreproof_mut_` prefix rather than a literal name list.
//!
//! ## Pre-registered interpretation of a miss
//!
//! If a mutation is not re-found inside its budget, the reading is that **this
//! component's site labelling is wrong** — a specific, one-round, fixable
//! problem — not that the bug is unfindable. That distinction is the point of
//! the exercise, and it is written down here before any result exists so it
//! cannot be chosen afterwards to suit one.
//!
//! ## What is deliberately NOT here
//!
//! #608 is OPEN. A mutation is a known-fixed defect by definition, so #608
//! cannot honestly be planted back and does not appear below. It enters the
//! pilot as a live hunt on the x86 lane: a find is a bonus and a miss is a
//! scoping datum. Either way it is reported as a hunt and never as a validation.

/// One planted defect.
pub struct Mutation {
    /// The cargo feature that re-introduces it.
    pub feature: &'static str,
    /// The issue whose defect this is.
    pub issue: &'static str,
    /// The pull request that fixed it.
    pub fixed_by: &'static str,
    /// The file carrying the re-introduction.
    pub site: &'static str,
    /// The predicate the harness expects to fire, as it appears in a
    /// `[COREPROOF:VIOLATION:...:pred=...]` record.
    pub predicate: &'static str,
}

/// Every planted defect, in the order the pilot's pass bar names them.
pub const REGISTER: &[Mutation] = &[
    Mutation {
        feature: "coreproof_mut_block_departure",
        issue: "#647",
        fixed_by: "PR #648",
        site: "kernel/src/task/scheduler.rs::Scheduler::block_current_inner",
        predicate: "BLOCKED_NOT_IN_READYQ",
    },
    Mutation {
        feature: "coreproof_mut_cpu_identity",
        issue: "#645",
        fixed_by: "PR #645",
        site: "kernel/src/arch_impl/aarch64/context_switch.rs::schedule_from_kernel",
        predicate: "CPU_IDENTITY_SPLIT",
    },
    Mutation {
        feature: "coreproof_mut_reclaim_bracket",
        issue: "#653",
        fixed_by: "PR #655",
        site: "kernel/src/task/process_task.rs::reclaim_deferred_process_resources",
        predicate: "TEARDOWN_COUNTERS",
    },
    Mutation {
        feature: "coreproof_mut_pending_next",
        issue: "#589",
        fixed_by: "PR #614",
        site: "kernel/src/task/scheduler.rs::Scheduler::resolve_pending_next_locked",
        predicate: "REDISPATCH_LIVENESS",
    },
    Mutation {
        feature: "coreproof_mut_futex_section",
        issue: "#584",
        fixed_by: "PR #604",
        site: "kernel/src/syscall/futex.rs::sys_futex_wait",
        predicate: "REDISPATCH_LIVENESS",
    },
    Mutation {
        feature: "coreproof_mut_masked_lock",
        issue: "#609",
        fixed_by: "PR #632",
        site: "kernel/src/task/scheduler.rs::release_reclaimed_threads",
        predicate: "PERCPU_STACK_ALIEN",
    },
];

/// The mutation this build carries, if any.
///
/// Exactly one is expected. Naming it in the run record is what stops a
/// mutation run and an unmutated run from being confused for each other later,
/// when only the serials survive.
pub fn armed() -> Option<&'static Mutation> {
    let feature = if cfg!(feature = "coreproof_mut_block_departure") {
        "coreproof_mut_block_departure"
    } else if cfg!(feature = "coreproof_mut_cpu_identity") {
        "coreproof_mut_cpu_identity"
    } else if cfg!(feature = "coreproof_mut_reclaim_bracket") {
        "coreproof_mut_reclaim_bracket"
    } else if cfg!(feature = "coreproof_mut_pending_next") {
        "coreproof_mut_pending_next"
    } else if cfg!(feature = "coreproof_mut_futex_section") {
        "coreproof_mut_futex_section"
    } else if cfg!(feature = "coreproof_mut_masked_lock") {
        "coreproof_mut_masked_lock"
    } else {
        return None;
    };
    REGISTER
        .iter()
        .find(|mutation| mutation.feature == feature)
}

/// The armed mutation's feature name for the run record, or `none`.
pub fn armed_name() -> &'static str {
    match armed() {
        Some(mutation) => mutation.feature,
        None => "none",
    }
}
