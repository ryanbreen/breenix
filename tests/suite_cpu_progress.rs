#[path = "../kernel/src/task/cpu_progress/record.rs"]
mod record;

use record::{line, Capture, CpuSample, Reason};

#[test]
fn receipt_keeps_the_interrupted_state_and_marks_missing_cpus() {
    let capture = Capture { sequence: 8, reporter: 42, reason: Reason::Pin, tick: 1200 };
    let row = CpuSample {
        sequence: 8,
        tick: 1201,
        tid: 71,
        name: *b"breenfs-commit\0\0\0\0\0\0\0\0\0\0",
        preempt: 1,
        interrupts: true,
        schedule_tick: 200,
        queue_len: 2,
        pinned_holds: 1,
        pc: 0x1abc,
        ..CpuSample::default()
    };
    assert_eq!(line(capture, 2, Some(row)).as_str(), concat!(
        "\n[SUITE_CPU_PROGRESS seq=8 reason=pin reporter=42 trigger_tick=1200 cpu=2 ",
        "source=nmi sample_tick=1201 tid=71 name=breenfs-commit preempt=0x1 if=1 ",
        "schedule_age_ticks=1001 rq=2 pinned_holds=1 pc=0x1abc]\n",
    ));
    let stale = CpuSample { sequence: 7, ..row };
    assert_eq!(line(capture, 3, Some(stale)).as_str(), concat!(
        "\n[SUITE_CPU_PROGRESS seq=8 reason=pin reporter=42 trigger_tick=1200 cpu=3 ",
        "source=unavailable]\n",
    ));
}

#[test]
fn largest_receipt_fits_and_thread_names_stay_on_one_line() {
    let capture = Capture {
        sequence: u64::MAX,
        reporter: u64::MAX,
        reason: Reason::Timeout,
        tick: u64::MAX,
    };
    let row = CpuSample {
        sequence: u64::MAX,
        tick: u64::MAX,
        tid: u64::MAX,
        name: *b"thread\n\r ]=\t0123456789AB",
        preempt: u32::MAX,
        interrupts: false,
        schedule_tick: 0,
        queue_len: u32::MAX,
        pinned_holds: u64::MAX,
        pc: u64::MAX,
        caller: true,
    };
    let receipt = line(capture, 7, Some(row));
    let text = receipt.as_str();
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains("reason=timeout"));
    assert!(text.contains("source=caller"));
    assert!(text.contains("name=thread______0123456789AB"));
    assert!(text.contains("preempt=0xffffffff if=0 schedule_age_ticks=18446744073709551615"));
    assert!(text.ends_with("rq=4294967295 pinned_holds=18446744073709551615 pc=0xffffffffffffffff]\n"));
}
