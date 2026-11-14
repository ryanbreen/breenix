#[path = "../../kernel/src/input/host/progress.rs"]
#[allow(dead_code)] // This host case exercises the recorder, not every hardware site.
mod progress;

use progress::{Count, FirstFailure, Lane, Progress, Site};

#[test]
fn first_overflow_preserves_progress_and_suspended_site_once() {
    let progress = Progress::new();
    let command = Lane::new();
    let failure = FirstFailure::new();
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
    assert!(failure.capture(|| (progress.sample(), command.site(), 256)));
    progress.note(Count::Push);
    drop(outer);
    for _ in 0..4096 {
        assert!(!failure.capture(|| unreachable!("later overflows must not resample")));
    }
    let (counts, site, queue_len) = failure.take().expect("first overflow must be retained");
    assert_eq!(counts[Count::Push as usize], 1);
    assert_eq!(counts[Count::Pop as usize], 1);
    assert_eq!(site, Site::Stream as u64);
    assert_eq!(queue_len, 256);
    assert_eq!(command.site(), Site::Idle as u64);
    assert!(
        failure.take().is_none(),
        "failure output must be bounded to one snapshot"
    );
}
