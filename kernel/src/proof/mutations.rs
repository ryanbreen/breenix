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
//! ## What round 3 measured, per mutation
//!
//! Round 2 proved all six mutation sites EXECUTE inside the measured window and
//! four of them still produced no violation. Under the interpretation above,
//! each miss was read as this component's labelling being wrong, and round 3
//! took each one apart. Two were exactly that and are fixed; two are not, and
//! saying which is which is the point of writing it down:
//!
//! * **#653** — fixed. The predicate was reading teardown counters the defect
//!   does not move. What the fix (PR #655) established is an INVARIANT — claim
//!   held implies bracket held — and `RECLAIM_CLAIM_UNBRACKETED` now scores it
//!   from the claim word and the per-CPU preempt counts, sampled in-window
//!   because the unbracketed interval does not survive quiescence.
//! * **#584** — fixed. The predicate named no futex marker at all, and the
//!   futex handoff oracle's stage-1 seam sat above the value check, where the
//!   split cannot lose its wake. The seam now sits immediately before the
//!   ENQUEUE — the same position in any build without the split — and
//!   `FUTEX_HANDOFF_RESCUED` reads the oracle's own `rescues` census.
//! * **#645** — NOT a labelling problem, and still missed. `CPU_IDENTITY_SPLIT`
//!   is the correct predicate and is already wired: `CpuId::current_checked` at
//!   the dispatch pivot records exactly the carried-versus-fresh disagreement
//!   the mutation reopens. What is missing is PRODUCTION of the state: the
//!   window is a handful of instructions inside a permanently seam-prohibited
//!   file, so nothing may label it, and the damage additionally requires the
//!   preempted thread to resume on ANOTHER CPU. Round 3 tried the only lever
//!   available from outside — a peer-side timer squeeze immediately before the
//!   scheduler entry — and measured no split in 5 seeds plus one manufactured
//!   `REDISPATCH_LIVENESS`, so it was reverted rather than kept as noise. The
//!   open lever is a migration-forcing arm, not another predicate.
//! * **#609** — NOT a labelling problem either, and the mutation is the reason.
//!   It removes the masked bracket around `drop(reclaimed_threads)`, which was
//!   link 1 of the chain in `609-RCA-RETRACTION-2026-08-21.md` §2.3 — but that
//!   link had TWO halves, and the other one (PR #632 typing
//!   `ARM64_STACK_BITMAP` as an `IrqSafeMutex`) is still in force here, so a
//!   holder cannot be preempted and the orphaned lock the field failure needs
//!   cannot form. The only interval the mutation reopens is the
//!   liveness-check-to-free window in `KernelStack::drop`, and it can only go
//!   stale if some CPU installs that slot's top inside it — which only the
//!   thread being reaped could do, and it is terminated. 15 mutated boots moved
//!   no existing census by a single field. A faithful re-introduction has to
//!   restore the bare `spin::Mutex` too, and would then present as #609
//!   presented: a wedged boot with no marker at all.
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
        predicate: "RECLAIM_CLAIM_UNBRACKETED",
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
        predicate: "FUTEX_HANDOFF_RESCUED",
    },
    Mutation {
        feature: "coreproof_mut_masked_lock",
        issue: "#609",
        fixed_by: "PR #645 with PR #632",
        site: "kernel/src/task/scheduler.rs::release_reclaimed_threads",
        predicate: "PERCPU_STACK_ALIEN",
    },
    // M7 (rung 2): the sibling entry to M6 above, not a rewrite of it. M6's
    // own miss and its documented reason (round 3's 15-mutated-boot
    // measurement, this module's own header) stay exactly as recorded —
    // this is the faithful completion `passbar.md` called for: the bare-
    // `spin::Mutex` half of #609's real defect, planted alongside M6's
    // unmasked drop so both halves of the original chain are present
    // together. Unlike every other entry, the expected outcome is NOT a
    // `[COREPROOF:VIOLATION:...]` line — #609 "presented as a wedged boot
    // with no marker at all," so the expected detection is the gate's own
    // missing-RUN-record condition. `predicate` names that explicitly rather
    // than leaving the field empty, so a future reader does not mistake "no
    // VIOLATION line" for a false negative.
    Mutation {
        feature: "coreproof_mut_masked_lock_bare",
        issue: "#609",
        fixed_by: "PR #632 (bitmap typing) with PR #645 (masking)",
        site: "kernel/src/memory/kernel_stack.rs::ARM64_STACK_BITMAP and \
               kernel/src/task/scheduler.rs::release_reclaimed_threads",
        predicate: "NONE_EXPECTED:missing_run_record_is_the_catch",
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
