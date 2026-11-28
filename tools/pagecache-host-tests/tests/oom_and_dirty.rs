//! E3-1: OOM victim order (plan section 13) and the dirty charge (11.5).
use pagecache_host_tests::dirty_limit::{DirtyCount, MountCharge, Throttle};
use pagecache_host_tests::oom_score::{select, Candidate, Chosen, Selection};

fn space(pid: u32, private_frames: u64, table_frames: u64) -> Candidate {
    Candidate {
        pid,
        private_frames,
        table_frames,
        holds_init: false,
        kernel_thread: false,
        chosen: Chosen::No,
    }
}

#[test]
fn victims_are_chosen_by_size_with_exclusions_and_ties() {
    let mut spaces = vec![
        Candidate { holds_init: true, ..space(1, 9000, 10) },
        space(10, 200, 0),
        space(20, 50, 0),
        space(30, 190, 10),
        Candidate { kernel_thread: true, ..space(40, 0, 8000) },
        space(50, 150, 49),
    ];
    // 10 and 30 tie at 200 (page tables count); the higher pid wins.
    assert_eq!(select(&spaces), Selection::Kill { victim: 3, next: [Some(1), Some(5)] });

    // No second victim while the first is exiting.
    spaces[3].chosen = Chosen::Exiting;
    assert_eq!(select(&spaces), Selection::Wait { victim: 3 });

    // A victim that overran its bound is passed over; selection repeats.
    spaces[3].chosen = Chosen::Overran;
    assert_eq!(select(&spaces), Selection::Kill { victim: 1, next: [Some(5), Some(2)] });

    // PID 1's space and kernel threads are not chosen, even when largest.
    let only_excluded = [spaces[0], spaces[4], spaces[3]];
    assert_eq!(select(&only_excluded), Selection::None);
    assert_eq!(select(&[]), Selection::None);
}

#[test]
fn a_charge_never_passes_the_limit_except_a_mount_with_nothing_charged() {
    // limit 100 with 30 frozen frames charged on the writers' mount; four
    // writers of 40 pages.
    let count = DirtyCount::new();
    let mount = MountCharge::new();
    count.charge_unchecked(&mount, 30);
    assert_eq!(count.try_charge(&mount, 40, 100), Ok(()));
    for _ in 0..3 {
        assert_eq!(count.try_charge(&mount, 40, 100), Err(Throttle { charged: 70, limit: 100 }));
    }
    assert_eq!((count.charged(), mount.charged()), (70, 70));
    // Writeback cleans 40: one waiting writer now fits, and the next does not.
    assert_eq!(count.release(&mount, 40), 30);
    assert_eq!(count.try_charge(&mount, 40, 100), Ok(()));
    assert_eq!(count.try_charge(&mount, 40, 100), Err(Throttle { charged: 70, limit: 100 }));

    // A part on a mount that holds no charge is admitted alone past the limit.
    let empty = MountCharge::new();
    assert_eq!(count.try_charge(&empty, 40, 100), Ok(()));
    assert_eq!((count.charged(), empty.charged()), (110, 40));
    // After that, a mount that holds dirty pages waits.
    assert!(count.try_charge(&empty, 1, 100).is_err());
    assert_eq!(empty.charged(), 40);
}

#[test]
fn two_writers_on_one_empty_mount_take_the_lone_part_exception_once() {
    // limit 120, with 100 charged by another mount; this mount holds nothing.
    let count = DirtyCount::new();
    count.charge_unchecked(&MountCharge::new(), 100);
    // Two writers to different files on the mount overlap (their io locks
    // are per file). The charge reads and claims the mount's count itself,
    // so the second writer meets the first's claim.
    let mount = MountCharge::new();
    assert_eq!(count.try_charge(&mount, 40, 120), Ok(()));
    assert_eq!(
        count.try_charge(&mount, 40, 120),
        Err(Throttle { charged: 140, limit: 120 })
    );
    assert_eq!(count.charged(), 140);
}
