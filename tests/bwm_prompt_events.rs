include!("bwm_validation.rs");

#[test]
fn unchanged_prompt_commit_does_not_schedule_another_readback() {
    use validation::{PromptCheck, ValidationQueue};
    let mut queue = ValidationQueue::new();
    queue.request(7);
    queue.presented(1);
    assert!(queue.next_prompt_if_idle(|_| Some(10), || true).is_some());
    assert_eq!(queue.prompt_result(7, 10, PromptCheck::Empty, 200), None);
    assert!(queue.next_prompt_if_idle(|_| Some(10), || true).is_none(),
        "an unchanged idle frame must not consume another full-screen readback");
}
