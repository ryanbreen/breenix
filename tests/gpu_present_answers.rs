extern crate alloc;
#[path = "../kernel/src/gpu/present_answers.rs"]
mod present_answers;
use present_answers::Answers;

#[test]
fn delayed_callers_keep_their_answers_after_more_than_four_completions() {
    let mut a = Answers::new();
    for n in 1..=12 { a.register(n); a.complete(n, 0x1100 + n); }
    for n in (1..=12).rev() { assert_eq!(a.take(n), Some(0x1100 + n)); }
    assert_eq!(a.take(1), None);
}

#[test]
fn interrupted_callers_do_not_accumulate_late_answers() {
    let mut a = Answers::new();
    a.register(1);
    assert_eq!(a.take(1), None);
    a.complete(1, 0x1100);
    assert_eq!(a.take(1), None);
    a.register(2);
    a.complete(2, 0x1100);
    assert_eq!(a.take(2), Some(0x1100));
}
