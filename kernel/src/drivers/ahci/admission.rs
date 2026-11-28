//! AHCI port admission's decision (docs/plans/e2-sleepable-contexts.md §6 and
//! §8, "E2.4: block I/O caller rule"), factored out of `begin_port_io` so
//! that a host check drives the production rule. The file uses only `core`
//! and the park contract's pure core, so `tools/park-host-tests` compiles it
//! unchanged.

use crate::block::read_failure::AhciReadError;
use crate::task::park_core::{classify, ContextInputs, ParkContext};

/// The architecture the decision is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

/// The architecture this kernel is built for.
#[cfg(target_arch = "x86_64")]
pub const ARCH: Arch = Arch::X86_64;
#[cfg(target_arch = "aarch64")]
pub const ARCH: Arch = Arch::Aarch64;

/// What `begin_port_io` does with a caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// The port has no owner: take it.
    Acquire,
    /// Another thread owns the port: park until it is released.
    Park,
    /// Spin once and look at the port again.
    Poll,
    /// Refuse the caller with this error.
    Refuse(AhciReadError),
}

/// Admission for a caller in `context` when the port's owner is `owner`
/// (0 for none).
pub const fn decide(context: ContextInputs, owner: u64, arch: Arch) -> Admission {
    if owner == 0 {
        return Admission::Acquire;
    }
    match classify(context) {
        ParkContext::Sleepable => Admission::Park,
        _ => match arch {
            Arch::X86_64 => Admission::Refuse(AhciReadError::CannotPark),
            Arch::Aarch64 => Admission::Poll,
        },
    }
}
