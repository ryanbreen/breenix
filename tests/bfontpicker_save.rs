#[path = "../userspace/programs/src/bfontpicker/conf.rs"]
mod conf;
#[path = "../userspace/programs/src/bfontpicker/save.rs"]
mod save;

#[test]
fn save_preserves_unknown_settings_and_reports_success() {
    let path = std::env::temp_dir().join(format!("bfontpicker-save-{}.conf", std::process::id()));
    let original = "# keep\nmono.font=old.ttf\nmono.size=14\nunknown.setting=keep=this\ndisplay.font=display.ttf\ndisplay.size=17\n";
    std::fs::write(&path, original).unwrap();
    let mut state = save::SaveState { path: path.clone(), save: None, error: None };
    assert!(state.update("mono.font", "new.ttf"), "{}", state.message());
    assert_eq!(state.save, Some("saved"));
    assert_eq!(state.error, None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original.replace("old.ttf", "new.ttf"));
    std::fs::remove_file(path).unwrap();
}
