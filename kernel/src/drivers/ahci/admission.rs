//! AHCI port admission's decision (docs/plans/e2-sleepable-contexts.md §6 and
//! §8, "E2.4: block I/O caller rule"), factored out of `begin_port_io` so
//! that a host check drives the production rule. The file uses only `core`
//! and the park contract's pure core, so `tools/park-host-tests` compiles it
//! unchanged.
//!
//! Block I/O requires a context that may park or the early-boot token
//! (`park_core::io_rule`). A busy port always parks a sleeping caller. Early
//! boot is the only thread and owns the controller, so a port another
//! thread owns there is a contract violation, refused. Any other context is
//! refused whatever the port's state: it gets no data, never a poll.

use crate::block::read_failure::AhciReadError;
use crate::task::park_core::{classify, io_rule, ContextInputs, IoContext};

/// What `begin_port_io` does with a caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// The port has no owner: take it, and wait for its commands as the
    /// context says (sleep, or poll in early boot).
    Acquire(IoContext),
    /// Another thread owns the port: park until it is released.
    Park,
    /// Refuse the caller with this error, after a receipt.
    Refuse(AhciReadError),
}

/// Admission for a caller in `context`, with the early-boot token held or
/// not, when the port's owner is `owner` (0 for none).
pub const fn decide(context: ContextInputs, token_held: bool, owner: u64) -> Admission {
    match io_rule(classify(context), token_held) {
        Err(_) => Admission::Refuse(AhciReadError::NotSleepable),
        Ok(io) if owner == 0 => Admission::Acquire(io),
        Ok(IoContext::Sleepable) => Admission::Park,
        Ok(IoContext::EarlyBoot) => Admission::Refuse(AhciReadError::EarlyBootPortOwned),
    }
}

/// The CPUs whose AHCI completion pollers start when CPU `arriving` comes
/// online (DEV-48(3)), as a mask over CPU ids below 64. `counted` is how many
/// CPUs the scheduler counts online: a CPU at or past it cannot take a
/// CPU-affine thread yet. `online` and `spawned` are the masks of the CPUs
/// marked online and of those whose poller has started.
///
/// Secondaries may arrive out of order, so the arrival that brings the count
/// past a CPU starts that CPU's poller too: each online CPU the count covers
/// and that has none, `arriving` included.
pub const fn pollers_to_start(arriving: usize, counted: usize, online: u64, spawned: u64) -> u64 {
    let covered = if counted >= 64 { u64::MAX } else { (1u64 << counted) - 1 };
    let arrived = if arriving < 64 { 1u64 << arriving } else { 0 };
    (online | arrived) & covered & !spawned
}
