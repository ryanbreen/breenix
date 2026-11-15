#[path = "../../kernel/src/input/host/progress.rs"]
#[allow(dead_code)] // This host case exercises the recorder, not every hardware site.
mod progress;

use progress::{Count, FirstFailure, Lane, Progress, Site};

#[test]
fn irq_overflow_fills_the_claimed_static_snapshot_in_place() {
    let recorder = include_str!("../../kernel/src/input/host/progress.rs");
    let diagnostics = include_str!("../../kernel/src/input/host/diagnostics.rs");
    let capture = diagnostics
        .split("pub fn capture_overflow(rx_len: usize)")
        .nth(1)
        .expect("production overflow capture")
        .split("pub fn report_failure()")
        .next()
        .unwrap();

    assert!(
        recorder.contains("sample: impl FnOnce(&mut T)"),
        "IRQ capture must receive the claimed slot, not return a Snapshot by value"
    );
    assert!(recorder.contains("value: UnsafeCell<T>"));
    assert!(recorder.contains("sample(&mut *self.value.get())"));
    assert!(diagnostics.contains("static FIRST: FirstFailure<Snapshot>"));
    assert!(capture.contains("FIRST.capture(|snapshot|"));
    assert!(capture.contains("snapshot.rows.iter_mut()"));
    assert!(
        !capture.contains("Snapshot {") && !capture.contains("[ThreadSample::empty"),
        "IRQ capture must not construct a Snapshot or its row array on the stack"
    );
}

#[test]
fn first_overflow_preserves_progress_and_suspended_site_once() {
    let progress = Progress::new();
    let command = Lane::new();
    let failure = FirstFailure::new(([0; progress::COUNT], 0));
    assert!(failure.take().is_none(), "passing input must remain silent");
    progress.note(Count::Push);
    progress.note(Count::Pop);
    assert_eq!(progress::NAMES.len(), progress.sample().len());
    let outer = command.enter(Site::Stream);
    {
        let _callback = command.enter(Site::Accept);
        assert_eq!(command.site(), Site::Accept as u64);
    }
    assert_eq!(command.site(), Site::Stream as u64);
    outer.at(Site::Stream);
    assert!(failure.capture(|snapshot| {
        snapshot.0 = progress.sample();
        snapshot.1 = command.site();
        assert!(failure.take().is_none(), "unfinished capture must stay private");
        assert!(!failure.capture(|_| unreachable!("slot is already claimed")));
    }));
    progress.note(Count::Push);
    drop(outer);
    for _ in 0..4096 {
        assert!(!failure.capture(|_| unreachable!("later overflows must not resample")));
    }
    let (counts, site) = failure.take().expect("first overflow must be retained");
    assert_eq!(counts[Count::Push as usize], 1);
    assert_eq!(counts[Count::Pop as usize], 1);
    assert_eq!(site, Site::Stream as u64);
    assert_eq!(command.site(), Site::Idle as u64);
    assert!(
        failure.take().is_none(),
        "failure output must be bounded to one snapshot"
    );
}
