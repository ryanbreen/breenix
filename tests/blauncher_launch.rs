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

#[test]
fn terminal_argv_runs_the_catalog_command() {
    let argv = vec!["/bin/echo".into(), "launched-terminal".into(), "two words".into()];
    assert_eq!(launch::terminal_argv(&argv), vec!["/usr/bin/bterm", "-e", "/bin/echo", "launched-terminal", "two words"]);
}

#[test]
fn launch_errors_name_missing_program_and_preserve_other_errors() {
    assert_eq!(launch::error_text("/usr/bin/no-such-app", true, "ENOENT"), "Missing program: /usr/bin/no-such-app");
    assert_eq!(launch::error_text("/bin/app", false, "EACCES"), "EACCES");
}
