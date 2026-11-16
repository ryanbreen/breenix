use breenfs_host_tests::{
    minimize::{minimize, Case, Fault},
    model::Model,
    Action, Fs,
};

/// Injected failure: a successful rename precedes a successful fsync, and the
/// crash image keeps write 1 of its window.
fn fails(case: &Case) -> bool {
    let Fault::Crash { selected, .. } = &case.fault else {
        return false;
    };
    let mut model = Model::new();
    let mut renamed = false;
    for op in case.operations() {
        let ok = model.execute(&op).is_ok();
        match op.action {
            Action::Rename { .. } if ok => renamed = true,
            Action::Fsync { .. } if ok && renamed => return selected.contains(&1),
            _ => (),
        }
    }
    false
}

#[test]
fn minimizer_reduces_a_failing_stream() {
    let fault = Fault::Crash {
        boundary: 0,
        writes: 3,
        selected: vec![0, 1, 2],
        tail: None,
        tear_seed: 0,
    };
    let case = (1..=64)
        .map(|seed| Case::seeded(seed, 256, fault.clone()))
        .find(fails)
        .expect("a seeded 256-operation stream renames before an fsync");
    let reduced = minimize(case.clone(), fails).unwrap();
    assert!(fails(&reduced), "{reduced:?}");

    // The rename, then the fsync last, plus required creates. Stream operations
    // name inodes by number, so the creates that allocate the numbers the rename
    // and fsync use are required too; the drop loop below shows each is needed.
    let ops = reduced.operations();
    let (last, rest) = ops.split_last().unwrap();
    assert!(matches!(last.action, Action::Fsync { .. }), "{reduced:?}");
    let renames = rest
        .iter()
        .filter(|op| matches!(op.action, Action::Rename { .. }))
        .count();
    assert_eq!(renames, 1, "{reduced:?}");
    for op in rest {
        assert!(
            matches!(
                op.action,
                Action::Rename { .. }
                    | Action::Create { .. }
                    | Action::Mkdir { .. }
                    | Action::Symlink { .. }
                    | Action::Link { .. }
            ),
            "not a create: {op:?} in {reduced:?}"
        );
    }
    // Truncated at the fsync; each kept operation is the stream's own.
    assert_eq!(reduced.keep.last(), Some(&(reduced.operation_count - 1)));
    let original = case.indexed_operations();
    for (index, op) in reduced.indexed_operations() {
        assert_eq!(original[index], (index, op));
    }
    assert!(matches!(&reduced.fault, Fault::Crash { selected, .. } if selected == &[1]));
    // No single operation or selected write can go.
    for at in 0..reduced.keep.len() {
        let mut fewer = reduced.clone();
        fewer.keep.remove(at);
        assert!(!fails(&fewer), "operation {at} was removable: {reduced:?}");
    }

    let row = reduced.regression_row();
    assert_eq!(
        row,
        format!(
            "Case {{ seed: {}, operation_count: {}, keep: vec!{:?}, fault: Fault::Crash {{ \
             boundary: 0, writes: 3, selected: vec![1], tail: None, \
             tear_seed: 0x0000000000000000 }} }}",
            reduced.seed, reduced.operation_count, reduced.keep
        )
    );
    assert_eq!(minimize(case, fails).unwrap(), reduced, "not deterministic");
    assert!(minimize(reduced.clone(), |_| false).is_err());
    let mut unordered = reduced;
    unordered.keep.reverse();
    assert!(minimize(unordered, |_| true).is_err());
}
