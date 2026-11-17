//! Pure block reservation accounting from design section 2.8.
//!
//! The mount supplies its current free-block count under its write lock. Grants
//! remain outstanding until their transaction is durable or discarded. Admission
//! here changes no trees and performs no allocation or I/O; callers must reserve
//! before mutation and retain the grant through publication.
use super::space::Error;

/// Host-only captures of refusals and allocation grants (E24 ledger 403/408, #3047).
#[cfg(test)]
pub mod observation {
    extern crate std;

    std::thread_local! {
        static REFUSAL: core::cell::Cell<Option<[u64; 5]>> = const { core::cell::Cell::new(None) };
        static WRITE_GRANT: core::cell::Cell<Option<[u64; 5]>> = const { core::cell::Cell::new(None) };
    }

    pub fn take() -> Option<[u64; 5]> {
        REFUSAL.with(|cell| cell.take())
    }

    pub(super) fn record(values: [u64; 5]) {
        REFUSAL.with(|cell| cell.set(Some(values)));
    }

    pub fn take_write_grant() -> Option<[u64; 5]> {
        WRITE_GRANT.with(|cell| cell.take())
    }

    pub(super) fn record_write_grant(values: [u64; 5]) {
        WRITE_GRANT.with(|cell| cell.set(Some(values)));
    }
}

/// Operation costs in blocks and nodes, before charging four blocks per node.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Estimate {
    pub data_blocks: u64,
    /// Distinct FS-path nodes not already copied in the transaction.
    pub uncopied_fs_nodes: u64,
    /// Levels on the affected FS paths, allowing one split at each level.
    pub fs_levels: u64,
    /// Space-item changes, including three per allocated node or extent.
    pub space_changes: u64,
    /// Space root's level (a leaf has height 0).
    pub space_height: u64,
    pub space_nodes: u64,
}

impl Estimate {
    pub fn blocks(self) -> Result<u64, Error> {
        // Wide intermediates retain the smaller bound even when the other
        // branch of min(changes * (height + 1), 2 * nodes + height) exceeds u64.
        let height = u128::from(self.space_height);
        let space_paths = u128::from(self.space_changes) * (height + 1);
        let space_tree = 2 * u128::from(self.space_nodes) + height;
        let fs_nodes = u128::from(self.uncopied_fs_nodes) + u128::from(self.fs_levels);
        let blocks = u128::from(self.data_blocks) + 4 * fs_nodes + 4 * space_paths.min(space_tree);
        u64::try_from(blocks).map_err(|_| Error::ENOSPC)
    }
}

/// Only operations that free space may consume the global reserve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Allocate,
    Unlink,
    Rmdir,
    /// Shrinking a file; an extension uses Allocate.
    Truncate,
    RemoveXattr,
    DeleteSnapshot,
}

/// A grant is consumed by release on its issuing ledger. It is intentionally
/// neither Copy nor Clone, so one grant cannot be returned twice.
#[derive(Debug)]
#[must_use = "retain the grant until its transaction is durable or discarded"]
pub struct Grant {
    blocks: u64,
}

impl Grant {
    pub fn blocks(&self) -> u64 {
        self.blocks
    }
}

/// A mount's reservations; synchronization belongs to the mount write lock.
pub struct Ledger {
    global_reserve: u64,
    outstanding: u64,
}

impl Ledger {
    /// Use the filesystem's global_reserve from its superblock (design 2.8).
    pub fn new(global_reserve: u64) -> Self {
        Self {
            global_reserve,
            outstanding: 0,
        }
    }

    pub fn outstanding(&self) -> u64 {
        self.outstanding
    }

    /// Unprivileged availability for f_bavail, in filesystem blocks.
    pub fn available(&self, free_blocks: u64) -> u64 {
        free_blocks
            .saturating_sub(self.global_reserve)
            .saturating_sub(self.outstanding)
    }

    /// Charge a precomputed estimate before the operation changes anything.
    /// free_blocks must include blocks backing outstanding grants: callers
    /// must not subtract the same granted blocks a second time here.
    pub fn reserve(
        &mut self,
        free_blocks: u64,
        blocks: u64,
        operation: Operation,
    ) -> Result<Grant, Error> {
        let available = match operation {
            Operation::Allocate => self.available(free_blocks),
            Operation::Unlink
            | Operation::Rmdir
            | Operation::Truncate
            | Operation::RemoveXattr
            | Operation::DeleteSnapshot => free_blocks.saturating_sub(self.outstanding),
        };
        if blocks > available {
            #[cfg(test)]
            observation::record([
                free_blocks,
                self.global_reserve,
                self.outstanding,
                blocks,
                self.available(free_blocks),
            ]);
            return Err(Error::ENOSPC);
        }
        let outstanding = self.outstanding.checked_add(blocks).ok_or(Error::ENOSPC)?;
        #[cfg(test)]
        if operation == Operation::Allocate {
            observation::record_write_grant([
                free_blocks, self.global_reserve, outstanding, blocks, available,
            ]);
        }
        self.outstanding = outstanding;
        Ok(Grant { blocks })
    }

    /// Return the exact grant after a durable commit or a discarded transaction.
    /// The grant must come from this ledger; release changes no free-block count.
    pub fn release(&mut self, grant: Grant) {
        self.outstanding = self
            .outstanding
            .checked_sub(grant.blocks)
            .expect("reservation released to the wrong ledger");
    }
}
