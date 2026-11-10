#[path = "../userspace/programs/src/blauncher/dismiss.rs"]
mod dismiss;

#[test]
fn dismissal_requires_focus_not_elapsed_time() {
    assert!(!dismiss::should_dismiss(false));
    assert!(dismiss::should_dismiss(true));
}
