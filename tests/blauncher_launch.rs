#[path = "../userspace/programs/src/blauncher/launch.rs"]
mod launch;

#[test]
fn gui_argv_preserves_each_field() {
    let argv = vec!["/usr/libexec/breenix/ack-app_test".into(), "--window".into(), "Ack".into(), "a".into(), "b c".into(), "--nonce".into(), "unit-nonce".into()];
    assert_eq!(launch::gui_argv(&argv), argv);
}

#[test]
fn gui_argv_preserves_empty_and_quoted_arguments() {
    let argv = vec!["/bin/app".into(), "".into(), "a\"b".into()];
    assert_eq!(launch::gui_argv(&argv), argv);
}
