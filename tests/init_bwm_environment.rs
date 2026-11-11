#[path = "../userspace/programs/src/init_bwm_environment.rs"]
mod init_bwm_environment;

#[test]
fn forwards_session_variables_and_preserves_production_environment() {
    use init_bwm_environment::bwm_environment;
    assert_eq!(bwm_environment(|_| None), Vec::<String>::new());
    let values = [
        ("BREENIX_UI_CLOCK", "/tmp/clock"),
        ("BREENIX_INPUT_DEVICES", "1,2"),
        ("BREENIX_CONFIG_ROOT", "/tmp/config"),
        ("BREENIX_UI_SEED", "42"),
        ("BREENIX_UI_MODE", "800x600@1"),
    ];
    let actual = bwm_environment(|key| values.iter().find(|(name, _)| *name == key).map(|(_, value)| value.to_string()));
    let expected: Vec<String> = values.iter().map(|(key, value)| format!("{key}={value}")).collect();
    assert_eq!(actual, expected);
}
