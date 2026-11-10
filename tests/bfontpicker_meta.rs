#[path = "../userspace/programs/src/bfontpicker/meta.rs"]
mod meta;

#[test]
fn shipped_fonts_are_classified_by_metadata() {
    let mono = ["CascadiaCode-Regular.ttf", "DejaVuSansMono.ttf", "FiraCode-Regular.ttf", "Hack-Regular.ttf", "IBMPlexMono-Regular.ttf", "Inconsolata-Regular.ttf", "JetBrainsMono-Regular.ttf", "NotoSansMono-Regular.ttf", "RobotoMono-Regular.ttf", "SourceCodePro-Regular.ttf", "UbuntuMono-Regular.ttf"];
    let mut seen = Vec::new();
    for entry in std::fs::read_dir("fonts").unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if !name.ends_with(".ttf") { continue; }
        let bytes = std::fs::read(entry.path()).unwrap();
        assert_eq!(meta::is_mono(&bytes).unwrap(), mono.contains(&name.as_str()), "{name}");
        seen.push(name);
    }
    assert_eq!(seen.len(), 29);
    for name in mono { assert!(seen.iter().any(|item| item == name), "missing {name}"); }
}

#[test]
fn renamed_font_and_unreadable_post() {
    let dir = std::env::temp_dir().join(format!("bfontpicker-meta-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("display.ttf");
    std::fs::copy("fonts/NotoSansMono-Regular.ttf", &path).unwrap();
    assert!(meta::is_mono(&std::fs::read(&path).unwrap()).unwrap());
    assert!(meta::is_mono(&std::fs::read(&path).unwrap()[..20]).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
