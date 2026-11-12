#[path = "../userspace/programs/src/blauncher/catalog.rs"]
mod catalog;

use catalog::{filter, parse, Mode};

#[test]
fn parses_good_rows_and_reports_each_bad_line() {
    let text = "# apps\n\nTerminal\tA terminal\tgui\t/usr/bin/bterm\t--new\nshort\trow\n\tdescription\tgui\t/bin/thing\nName\tdescription\tother\t/bin/thing\nName\tdescription\tterminal\trelative\nShell\tCommand line\tterminal\t/bin/bsh\n";
    let (entries, errors) = parse(text);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "Terminal");
    assert_eq!(entries[0].description, "A terminal");
    assert_eq!(entries[0].mode, Mode::Gui);
    assert_eq!(entries[0].argv, ["/usr/bin/bterm", "--new"]);
    assert_eq!(entries[1].mode, Mode::Terminal);
    assert_eq!(errors.iter().map(|e| (e.line, e.reason.as_str())).collect::<Vec<_>>(), [
        (4, "too few fields"), (5, "empty name"), (6, "unknown mode"), (7, "relative argv[0]"),
    ]);
}

#[test]
fn filters_name_or_description_in_catalog_order() {
    let (entries, errors) = parse("Alpha\tSystem LOG viewer\tgui\t/bin/a\nLogbook\tOther\tterminal\t/bin/b\nThird\tElse\tgui\t/bin/c\n");
    assert!(errors.is_empty());
    let names = |query| filter(&entries, query).iter().map(|e| e.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names("lOg"), ["Alpha", "Logbook"]);
    assert_eq!(names(""), ["Alpha", "Logbook", "Third"]);
    assert!(names("missing").is_empty());
}

#[test]
fn shipped_catalog_parses_without_errors() {
    let text = std::fs::read_to_string("userspace/programs/share/applications/catalog").unwrap();
    let (entries, errors) = parse(&text);
    assert!(!entries.is_empty());
    let icon_kit = entries.iter().find(|entry| entry.name == "Breenix Icon Kit").expect("Breenix Icon Kit is installed in the launcher");
    assert_eq!(icon_kit.mode, Mode::Gui);
    assert_eq!(icon_kit.argv, ["/usr/bin/biconkit"]);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn shipped_catalog_modes_match_installed_programs() {
    let text = std::fs::read_to_string("userspace/programs/share/applications/catalog").unwrap();
    let (entries, errors) = parse(&text);
    assert!(errors.is_empty(), "{errors:?}");
    let build = std::fs::read_to_string("userspace/programs/build.sh").unwrap();
    let binaries = build.split_once("STD_BINARIES=(\n").unwrap().1.split_once("\n)").unwrap().0;
    for entry in entries {
        let program = &entry.argv[0];
        if entry.mode != Mode::Gui && !program.starts_with("/usr/bin/") {
            continue;
        }
        let name = program.strip_prefix("/usr/bin/").expect("GUI program must be in /usr/bin");
        if entry.mode == Mode::Gui {
            assert!(binaries.lines().any(|line| line.trim() == format!("\"{name}\"")), "{}: {program} is not installed by build.sh", entry.name);
        }
        let source = std::fs::read_to_string(format!("userspace/programs/src/{name}.rs"))
            .unwrap_or_else(|e| panic!("{}: source for {program}: {e}", entry.name));
        let creates_window = source.contains("Window::new(") || source.contains("Window::new_without_fonts(");
        assert_eq!(creates_window, entry.mode == Mode::Gui, "{}: {program} mode does not match its window creation", entry.name);
    }
}
