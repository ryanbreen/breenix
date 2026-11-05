#![cfg(test)]

#[path = "../../../kernel/src/memory/table_reclaim.rs"]
mod table_reclaim;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::rc::Rc;
use table_reclaim::{reclaim_range, TableOps};

const PAGE: u64 = 1 << 12;
const M2: u64 = 1 << 21;
const G1: u64 = 1 << 30;
const G512: u64 = 1 << 39;
const ROOT: usize = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Empty,
    Table(usize),
    Leaf,
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Unlink(usize),
    Return(usize),
}

#[derive(Default)]
struct State {
    tables: BTreeMap<usize, [Entry; 512]>,
    returned: BTreeSet<usize>,
    events: Vec<Event>,
}

struct Mock(Rc<RefCell<State>>);

impl Mock {
    fn new() -> Self {
        let mut state = State::default();
        state.tables.insert(ROOT, [Entry::Empty; 512]);
        Self(Rc::new(RefCell::new(state)))
    }

    // Allocate the three intermediate tables; leave the final leaf empty.
    fn path(&mut self, address: u64) -> [usize; 3] {
        let mut state = self.0.borrow_mut();
        let mut parent = ROOT;
        let mut path = [ROOT; 3];
        for (depth, shift) in [39, 30, 21].into_iter().enumerate() {
            let index = ((address >> shift) & 511) as usize;
            let child = match state.tables[&parent][index] {
                Entry::Table(child) => child,
                Entry::Empty => {
                    let child = state.tables.len();
                    state.tables.insert(child, [Entry::Empty; 512]);
                    state.tables.get_mut(&parent).unwrap()[index] = Entry::Table(child);
                    child
                }
                Entry::Leaf => panic!("a block mapping is not a table"),
            };
            path[depth] = child;
            parent = child;
        }
        path
    }

    fn set_leaf(&mut self, table: usize, index: usize) {
        self.0.borrow_mut().tables.get_mut(&table).unwrap()[index] = Entry::Leaf;
    }

    fn run(&mut self, unmapped: Range<u64>, bounds: Range<u64>, user_end: u64) -> Vec<usize> {
        let state = Rc::clone(&self.0);
        let mut frames = Vec::new();
        let count = reclaim_range(self, ROOT, unmapped, bounds, user_end, |frame| {
            let mut state = state.borrow_mut();
            assert_ne!(frame, ROOT, "the root must stay allocated");
            assert!(state.tables[&frame].iter().all(|e| *e == Entry::Empty));
            assert!(
                state
                    .tables
                    .values()
                    .flatten()
                    .all(|e| *e != Entry::Table(frame)),
                "frame {frame} is still linked when returned"
            );
            assert_eq!(state.events.last(), Some(&Event::Unlink(frame)));
            assert!(state.returned.insert(frame), "frame returned twice");
            state.events.push(Event::Return(frame));
            frames.push(frame);
        });
        assert_eq!(count, frames.len());
        frames
    }
}

impl TableOps for Mock {
    type Table = usize;
    type Frame = usize;

    fn child_table(&self, table: usize, index: usize) -> Option<usize> {
        let state = self.0.borrow();
        assert!(!state.returned.contains(&table), "walk after return");
        match state.tables[&table][index] {
            Entry::Table(child) => Some(child),
            Entry::Empty | Entry::Leaf => None,
        }
    }

    fn entry_empty(&self, table: usize, index: usize) -> bool {
        let state = self.0.borrow();
        assert!(!state.returned.contains(&table), "read after return");
        state.tables[&table][index] == Entry::Empty
    }

    fn unlink(&mut self, parent: usize, index: usize) -> usize {
        let mut state = self.0.borrow_mut();
        assert!(
            !state.returned.contains(&parent),
            "unlink from returned parent"
        );
        let Entry::Table(frame) = state.tables[&parent][index] else {
            panic!("unlink of a non-table descriptor");
        };
        assert!(state.tables[&frame].iter().all(|e| *e == Entry::Empty));
        state.tables.get_mut(&parent).unwrap()[index] = Entry::Empty;
        state.events.push(Event::Unlink(frame));
        frame
    }
}

#[test]
fn unused_windows_are_returned_bottom_up_after_unlink() {
    let mut ops = Mock::new();
    let [upper, middle, first] = ops.path(M2 - PAGE);
    let [_, _, second] = ops.path(M2);
    // The unmap touches only the boundary pages, but its neighbours leave the
    // entire 512 GiB window unused, including the two partial 2 MiB windows.
    assert_eq!(
        ops.run(M2 - PAGE..M2 + PAGE, 0..G512, 1 << 47),
        [first, second, middle, upper]
    );
    assert_eq!(ops.0.borrow().tables[&ROOT], [Entry::Empty; 512]);
}

#[test]
fn neighbour_vmas_keep_their_shared_window_even_if_descriptors_are_empty() {
    for bounds in [PAGE..G512, 0..M2 - PAGE] {
        let mut ops = Mock::new();
        ops.path(2 * PAGE);
        assert!(ops.run(2 * PAGE..3 * PAGE, bounds, 1 << 47).is_empty());
    }
    // An exact boundary does not share the next window.
    let mut ops = Mock::new();
    let [_, _, leaf] = ops.path(M2);
    assert_eq!(ops.run(M2..M2 + PAGE, M2..2 * M2, 1 << 47), [leaf]);
}

#[test]
fn crossing_one_gib_frees_each_upper_table_only_after_it_becomes_empty() {
    for keep_second in [false, true] {
        let mut ops = Mock::new();
        let [upper, left_middle, left_leaf] = ops.path(G1 - PAGE);
        let [_, right_middle, right_leaf] = ops.path(G1);
        if keep_second {
            ops.set_leaf(right_middle, 511); // A live 2 MiB block elsewhere.
        }
        let expected = if keep_second {
            vec![left_leaf, left_middle, right_leaf]
        } else {
            vec![left_leaf, left_middle, right_leaf, right_middle]
        };
        assert_eq!(ops.run(G1 - PAGE..G1 + PAGE, 0..2 * G1, 1 << 47), expected);
        assert!(!ops.0.borrow().returned.contains(&upper)); // Ceiling cuts its span.
    }
}

#[test]
fn nonempty_tables_are_kept_including_entries_outside_the_unmap() {
    let mut ops = Mock::new();
    let [_, _, leaf] = ops.path(PAGE);
    ops.set_leaf(leaf, 511);
    assert!(ops.run(PAGE..2 * PAGE, 0..G512, 1 << 47).is_empty());
}

#[test]
fn table_windows_outside_the_unmap_are_not_visited() {
    let mut ops = Mock::new();
    let [_, _, first] = ops.path(PAGE);
    let [upper, middle, second] = ops.path(M2);
    assert_eq!(ops.run(PAGE..2 * PAGE, 0..G512, 1 << 47), [first]);
    let state = ops.0.borrow();
    for kept in [upper, middle, second] {
        assert!(!state.returned.contains(&kept));
    }
}

#[test]
fn block_descriptors_are_not_walked_or_unlinked() {
    let mut ops = Mock::new();
    let [upper, middle, leaf] = ops.path(PAGE);
    ops.set_leaf(upper, 1); // A 1 GiB block, rather than a table.
    assert_eq!(ops.run(PAGE..2 * G1, 0..G512, 1 << 47), [leaf, middle]);
    assert_eq!(ops.0.borrow().tables[&upper][1], Entry::Leaf);
}

#[test]
fn user_limit_clamps_the_walk_and_ceiling_for_both_address_spaces() {
    for user_end in [1 << 47, 1 << 48] {
        let mut ops = Mock::new();
        let [upper, middle, leaf] = ops.path(user_end - PAGE);
        if user_end == 1 << 47 {
            ops.set_leaf(ROOT, 256); // The kernel half is out of bounds.
        }
        assert_eq!(
            ops.run(user_end - PAGE..u64::MAX, 0..u64::MAX, user_end),
            [leaf, middle, upper]
        );
        assert_eq!(ops.0.borrow().returned.len(), 3);
    }
    // A user limit inside a table window cannot authorize freeing that table.
    let mut ops = Mock::new();
    ops.path(PAGE);
    assert!(ops.run(PAGE..u64::MAX, 0..u64::MAX, M2 - PAGE).is_empty());
}

#[test]
fn empty_reversed_or_out_of_user_ranges_do_not_unlink() {
    let mut ops = Mock::new();
    ops.path(PAGE);
    for (start, end, floor, ceiling, user_end) in [
        (PAGE, PAGE, 0, G512, 1 << 47),
        (2 * PAGE, PAGE, 0, G512, 1 << 47),
        (PAGE, 2 * PAGE, M2, PAGE, 1 << 47),
        (1 << 47, u64::MAX, 0, u64::MAX, 1 << 47),
        (PAGE, 2 * PAGE, 0, G512, 0),
    ] {
        assert!(ops.run(start..end, floor..ceiling, user_end).is_empty());
    }
    assert!(ops.0.borrow().events.is_empty());
}
