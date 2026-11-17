use breenfs_host_tests::{
    reserve::{Estimate, Ledger, Operation},
    space::Error,
};

#[test]
fn estimate_counts_data_and_four_block_nodes() {
    let cases = [
        (Estimate::default(), Ok(0)),
        (
            Estimate {
                data_blocks: 7,
                ..Estimate::default()
            },
            Ok(7),
        ),
        (
            Estimate {
                uncopied_fs_nodes: 3,
                fs_levels: 2,
                ..Estimate::default()
            },
            Ok(20),
        ),
        (
            Estimate {
                space_changes: 3,
                space_height: 2,
                space_nodes: 100,
                ..Estimate::default()
            },
            Ok(36),
        ),
        (
            Estimate {
                space_changes: 100,
                space_height: 2,
                space_nodes: 4,
                ..Estimate::default()
            },
            Ok(40),
        ),
        (
            Estimate {
                data_blocks: 11,
                uncopied_fs_nodes: 5,
                fs_levels: 3,
                space_changes: 27,
                space_height: 2,
                space_nodes: 4,
            },
            Ok(83),
        ),
        (
            Estimate {
                data_blocks: u64::MAX,
                ..Estimate::default()
            },
            Ok(u64::MAX),
        ),
        (
            Estimate {
                data_blocks: u64::MAX - 4,
                uncopied_fs_nodes: 1,
                ..Estimate::default()
            },
            Ok(u64::MAX),
        ),
        (
            Estimate {
                uncopied_fs_nodes: u64::MAX,
                ..Estimate::default()
            },
            Err(Error::ENOSPC),
        ),
        // The finite whole-space-tree bound wins even when the path product exceeds u64.
        (
            Estimate {
                space_changes: u64::MAX,
                space_height: 7,
                space_nodes: 1,
                ..Estimate::default()
            },
            Ok(36),
        ),
        (
            Estimate {
                space_changes: 1,
                space_height: u64::MAX,
                space_nodes: u64::MAX,
                ..Estimate::default()
            },
            Err(Error::ENOSPC),
        ),
    ];
    for (estimate, expected) in cases {
        assert_eq!(estimate.blocks(), expected, "{estimate:?}");
    }
}

#[test]
fn grants_charge_outstanding_and_refusals_change_nothing() {
    let mut ledger = Ledger::new(10);
    let mut grants = Vec::new();
    for (operation, blocks, expected, outstanding, available) in [
        (Operation::Allocate, 20, Ok(20), 20, 10),
        (Operation::Allocate, 11, Err(Error::ENOSPC), 20, 10),
        (Operation::Allocate, 10, Ok(10), 30, 0),
        (Operation::Allocate, 1, Err(Error::ENOSPC), 30, 0),
        (Operation::Unlink, 10, Ok(10), 40, 0),
        (Operation::Rmdir, 1, Err(Error::ENOSPC), 40, 0),
    ] {
        let result = ledger.reserve(40, blocks, operation);
        assert_eq!(
            result.as_ref().map(|grant| grant.blocks()).map_err(|e| *e),
            expected
        );
        assert_eq!(ledger.outstanding(), outstanding);
        assert_eq!(ledger.available(40), available);
        if let Ok(grant) = result {
            grants.push(grant);
        }
    }
    for grant in grants {
        ledger.release(grant);
    }
    assert_eq!(ledger.outstanding(), 0);
    assert_eq!(ledger.available(40), 30);
}

#[test]
fn freeing_operations_use_the_reserve_but_cannot_exceed_free_blocks() {
    for operation in [
        Operation::Unlink,
        Operation::Rmdir,
        Operation::Truncate,
        Operation::RemoveXattr,
        Operation::DeleteSnapshot,
    ] {
        for (free, global_reserve) in [(16, 16), (7, 16)] {
            let mut ledger = Ledger::new(global_reserve);
            assert_eq!(
                ledger.reserve(free, 1, Operation::Allocate).err(),
                Some(Error::ENOSPC)
            );
            assert_eq!(ledger.outstanding(), 0);
            let grant = ledger.reserve(free, free, operation).unwrap();
            assert_eq!(grant.blocks(), free);
            assert_eq!(ledger.outstanding(), free);
            assert_eq!(ledger.available(free), 0);
            assert_eq!(
                ledger.reserve(free, 1, operation).err(),
                Some(Error::ENOSPC)
            );
            assert_eq!(ledger.outstanding(), free);
            ledger.release(grant);
            assert_eq!(ledger.outstanding(), 0);
        }
    }
}

#[test]
fn durable_and_discarded_transactions_release_their_exact_grants() {
    // Both transaction outcomes return their held grants through the same release API.
    for (durable, discarded) in [(13, 29), (29, 13)] {
        let mut ledger = Ledger::new(8);
        let committed = ledger.reserve(100, durable, Operation::Allocate).unwrap();
        let abandoned = ledger.reserve(100, discarded, Operation::Allocate).unwrap();
        assert_eq!(ledger.outstanding(), durable + discarded);
        ledger.release(committed);
        assert_eq!(ledger.outstanding(), discarded);
        assert_eq!(ledger.available(100), 100 - 8 - discarded);
        ledger.release(abandoned);
        assert_eq!(ledger.outstanding(), 0);
        assert_eq!(ledger.available(100), 92);
    }
}

#[test]
fn available_saturates_when_free_space_is_below_reserve_or_outstanding() {
    for (free, reserve, expected) in [
        (0, 0, 0),
        (7, 16, 0),
        (16, 16, 0),
        (17, 16, 1),
        (u64::MAX, 0, u64::MAX),
        (u64::MAX, u64::MAX, 0),
    ] {
        assert_eq!(Ledger::new(reserve).available(free), expected);
    }
    let mut ledger = Ledger::new(10);
    let grant = ledger.reserve(100, 80, Operation::Allocate).unwrap();
    for (free, expected) in [(100, 10), (90, 0), (80, 0), (5, 0), (0, 0)] {
        assert_eq!(ledger.available(free), expected);
    }
    assert_eq!(
        ledger.reserve(5, 1, Operation::Unlink).err(),
        Some(Error::ENOSPC)
    );
    assert_eq!(ledger.outstanding(), 80);
    ledger.release(grant);
    assert_eq!(ledger.available(100), 90);
}
