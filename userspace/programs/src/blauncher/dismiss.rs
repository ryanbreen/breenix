/// Ignore startup dismissal events until the launcher has gained focus once.
pub fn should_dismiss(has_focus: bool) -> bool {
    has_focus
}
