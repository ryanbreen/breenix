//! E3-1: the 140 (state, event) pairs of the page-state table (plan section 5.2).
use pagecache_host_tests::page_state::{
    transition, EntryChange, Event, PageState, Refused, Transition, NOT_ADMITTED,
};

use EntryChange::{Insert, Keep, Remove, Replace};
use PageState::{Absent, Dirty, Filling, Hole, Uptodate, Writeback};

/// Generation 7 is the one whose pages the states name; 8 follows it.
const STATES: [PageState; 7] = [
    Absent,
    Filling,
    Uptodate,
    Dirty { gen: NOT_ADMITTED },
    Dirty { gen: 7 },
    Writeback { gen: 7 },
    Hole,
];

const EVENTS: [Event; 20] = [
    Event::ClaimFill,
    Event::FillData,
    Event::FillHole,
    Event::FillFailed,
    Event::Cancel,
    Event::Write { open: NOT_ADMITTED },
    Event::Write { open: 7 },
    Event::Write { open: 8 },
    Event::SharedStore,
    Event::MappedAdmission { open: NOT_ADMITTED },
    Event::MappedAdmission { open: 8 },
    Event::Freeze { gen: NOT_ADMITTED },
    Event::Freeze { gen: 7 },
    Event::Freeze { gen: 8 },
    Event::Adopted { gen: NOT_ADMITTED },
    Event::Adopted { gen: 7 },
    Event::Adopted { gen: 8 },
    Event::Reclaim,
    Event::Truncate,
    Event::EofGrow,
];

/// The legal transitions, written from the plan's table; the other pairs are
/// refused.
fn legal() -> Vec<(PageState, Event, PageState, EntryChange)> {
    let d0 = Dirty { gen: NOT_ADMITTED };
    let d7 = Dirty { gen: 7 };
    let d8 = Dirty { gen: 8 };
    let w7 = Writeback { gen: 7 };
    vec![
        (Absent, Event::ClaimFill, Filling, Insert),
        (Absent, Event::Write { open: 7 }, d7, Insert),
        (Absent, Event::Write { open: 8 }, d8, Insert),
        (Absent, Event::Truncate, Absent, Keep),
        (Absent, Event::EofGrow, Hole, Insert),
        (Filling, Event::FillData, Uptodate, Keep),
        (Filling, Event::FillHole, Hole, Replace),
        (Filling, Event::FillFailed, Absent, Remove),
        (Filling, Event::Cancel, Absent, Remove),
        (Filling, Event::Truncate, Absent, Remove),
        (Uptodate, Event::Write { open: 7 }, d7, Keep),
        (Uptodate, Event::Write { open: 8 }, d8, Keep),
        (Uptodate, Event::SharedStore, d0, Keep),
        (Uptodate, Event::Reclaim, Absent, Remove),
        (Uptodate, Event::Truncate, Absent, Remove),
        (d0, Event::Write { open: 7 }, d7, Keep),
        (d0, Event::Write { open: 8 }, d8, Keep),
        (d0, Event::SharedStore, d0, Keep),
        (d0, Event::MappedAdmission { open: 8 }, d8, Keep),
        (d0, Event::Truncate, Absent, Remove),
        (d7, Event::Write { open: 7 }, d7, Keep),
        (d7, Event::SharedStore, d7, Keep),
        (d7, Event::Freeze { gen: 7 }, w7, Keep),
        (d7, Event::Truncate, Absent, Remove),
        (w7, Event::Write { open: 8 }, d8, Replace),
        (w7, Event::SharedStore, d0, Replace),
        (w7, Event::Adopted { gen: 7 }, Uptodate, Keep),
        (w7, Event::Truncate, Absent, Remove),
        (Hole, Event::Write { open: 7 }, d7, Replace),
        (Hole, Event::Write { open: 8 }, d8, Replace),
        (Hole, Event::SharedStore, d0, Replace),
        (Hole, Event::Reclaim, Absent, Remove),
        (Hole, Event::Truncate, Absent, Remove),
    ]
}

#[test]
fn every_state_and_event_pair_is_legal_or_refused_as_the_table_says() {
    let legal = legal();
    let mut seen = 0;
    for &from in &STATES {
        for &event in &EVENTS {
            let expected = legal
                .iter()
                .find(|(f, e, _, _)| *f == from && *e == event)
                .map(|&(_, _, to, entry)| Transition { to, entry });
            match expected {
                Some(t) => {
                    seen += 1;
                    assert_eq!(transition(from, event), Ok(t), "{from:?} + {event:?}");
                }
                None => assert_eq!(
                    transition(from, event),
                    Err(Refused { from, event }),
                    "{from:?} + {event:?} must be refused"
                ),
            }
        }
    }
    assert_eq!(seen, legal.len(), "a legal row names a pair outside the sample");
}

