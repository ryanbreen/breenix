//! Deterministic reduction of a failing crash or bit-flip case into a
//! replayable regression row (breenfs design section 5: failing seeds are
//! minimized into regression tests). The minimizer is `no_std` library code;
//! the caller supplies the failure predicate, so a host binary can drive it
//! with the same acceptance a test uses.
use crate::{corrupt::Flip, crash::TornTail, stream::Stream, Errno, Operation, Result};
use alloc::{format, string::String, vec::Vec};

/// Where the failure happens after the case's operations have run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Fault {
    /// A crash image at flush boundary `boundary` of the case's run (its final
    /// clean remount included). `writes` is that window's 512-byte write count,
    /// and `selected` the ascending indices of the writes the image keeps.
    /// `tail` and `tear_seed` describe a torn variant (crash.rs).
    Crash {
        boundary: usize,
        writes: usize,
        selected: Vec<usize>,
        tail: Option<TornTail>,
        tear_seed: u64,
    },
    /// One bit flipped in a committed image (corrupt.rs).
    Flip(Flip),
}

/// A seeded stream reduced to the kept operations, and its fault.
/// `keep` holds ascending indices into `Stream::new(seed).take(operation_count)`,
/// so each kept operation is the seeded stream's own, unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Case {
    pub seed: u64,
    pub operation_count: usize,
    pub keep: Vec<usize>,
    pub fault: Fault,
}

/// What replaying a case found. A failure of the acceptance itself panics or
/// returns an error, as in the corpus runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The case was replayed and passed the corpus acceptance.
    Accepted,
    /// The fault names a boundary, write count, operation or block that the
    /// case's run does not have, and the case was not checked.
    Inapplicable,
}

impl Case {
    /// A case keeping the stream's first `operation_count` operations.
    pub fn seeded(seed: u64, operation_count: usize, fault: Fault) -> Self {
        Self {
            seed,
            operation_count,
            keep: (0..operation_count).collect(),
            fault,
        }
    }

    /// The kept operations with their indices in the seeded stream.
    pub fn indexed_operations(&self) -> Vec<(usize, Operation)> {
        let mut keep = self.keep.iter().peekable();
        Stream::new(self.seed)
            .take(self.operation_count)
            .enumerate()
            .filter(|(index, _)| keep.next_if_eq(&index).is_some())
            .collect()
    }

    pub fn operations(&self) -> Vec<Operation> {
        self.indexed_operations()
            .into_iter()
            .map(|(_, op)| op)
            .collect()
    }

    /// Indices ascend without repeats and lie inside their ranges.
    pub fn well_formed(&self) -> bool {
        let ascending = |v: &[usize], end: usize| {
            v.windows(2).all(|w| w[0] < w[1]) && v.last().map_or(true, |&last| last < end)
        };
        ascending(&self.keep, self.operation_count)
            && match &self.fault {
                Fault::Crash {
                    writes, selected, ..
                } => ascending(selected, *writes),
                Fault::Flip(_) => true,
            }
    }

    /// The case as a Rust expression for the table in tests/crash_regressions.rs,
    /// which imports `corrupt`, `crash` and `minimize::{Case, Fault}`.
    pub fn regression_row(&self) -> String {
        let fault = match &self.fault {
            Fault::Crash {
                boundary,
                writes,
                selected,
                tail,
                tear_seed,
            } => format!(
                "Fault::Crash {{ boundary: {boundary}, writes: {writes}, selected: vec!{selected:?}, tail: {}, tear_seed: {tear_seed:#018x} }}",
                match tail {
                    None => "None",
                    Some(TornTail::Old) => "Some(crash::TornTail::Old)",
                    Some(TornTail::Garbage) => "Some(crash::TornTail::Garbage)",
                }
            ),
            Fault::Flip(flip) => format!(
                "Fault::Flip(corrupt::Flip {{ seed: {:#x}, operation: {:?}, block: {}, bit: {}, kind: corrupt::Kind::{:?} }})",
                flip.seed, flip.operation, flip.block, flip.bit, flip.kind
            ),
        };
        format!(
            "Case {{ seed: {}, operation_count: {}, keep: vec!{:?}, fault: {fault} }}",
            self.seed, self.operation_count, self.keep
        )
    }
}

/// Shrink a failing case while `fails` keeps reporting the same failure:
/// 1. truncate the stream to its shortest failing prefix;
/// 2. drop single operations, latest first, until no single drop still fails;
/// 3. drop single selected crash writes the same way.
///
/// The result fails, and removing any one of its operations or selected writes
/// does not. The predicate decides what counts as the same failure, and returns
/// false for a candidate it cannot apply. The input must be well formed and
/// must fail; otherwise this returns EINVAL. Equal inputs give equal results.
pub fn minimize(case: Case, mut fails: impl FnMut(&Case) -> bool) -> Result<Case> {
    if !case.well_formed() || !fails(&case) {
        return Err(Errno::EINVAL.into());
    }
    let mut best = case;
    for count in 0..best.operation_count {
        let mut candidate = best.clone();
        candidate.operation_count = count;
        candidate.keep.retain(|&index| index < count);
        if fails(&candidate) {
            best = candidate;
            break;
        }
    }
    best = drop_singles(best, &mut fails, |case| &mut case.keep);
    if matches!(best.fault, Fault::Crash { .. }) {
        best = drop_singles(best, &mut fails, |case| match &mut case.fault {
            Fault::Crash { selected, .. } => selected,
            Fault::Flip(_) => unreachable!("checked above"),
        });
    }
    Ok(best)
}

fn drop_singles(
    mut best: Case,
    fails: &mut impl FnMut(&Case) -> bool,
    list: impl Fn(&mut Case) -> &mut Vec<usize>,
) -> Case {
    loop {
        let mut changed = false;
        let mut at = list(&mut best).len();
        while at > 0 {
            at -= 1;
            let mut candidate = best.clone();
            list(&mut candidate).remove(at);
            if fails(&candidate) {
                best = candidate;
                changed = true;
            }
        }
        if !changed {
            return best;
        }
    }
}
