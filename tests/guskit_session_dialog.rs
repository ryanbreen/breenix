#[path = "../userspace/programs/src/guskit/session_dialog.rs"]
mod session_dialog;
use session_dialog::{Dialog, Mode, Outcome};

#[test]
fn validates_and_cancels_session_requests() {
    let mut join = Dialog::new(Mode::Join);
    assert_eq!(join.submit("10.0.2.2:9000", "artist"), Outcome::Start(false, [10, 0, 2, 2], 9000, "artist".into()));
    let mut join = Dialog::new(Mode::Join);
    assert_eq!(join.submit("bad address", "artist"), Outcome::Invalid);
    assert!(!join.status().is_empty());
    assert_eq!(join.submit("10.0.2.2", ""), Outcome::Invalid);
    assert!(!join.status().is_empty());
    assert_eq!(join.cancel(), Outcome::Cancelled);
    let mut host = Dialog::new(Mode::Host);
    assert_eq!(host.submit("", "artist"), Outcome::Start(true, [0, 0, 0, 0], libcollab::endpoint::COLLAB_PORT, "artist".into()));
}
