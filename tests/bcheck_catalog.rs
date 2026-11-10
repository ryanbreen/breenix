#[path = "../userspace/programs/src/bcheck/catalog.rs"]
mod catalog;

#[test]
fn parses_entries_and_keeps_bad_lines_as_failed_rows() {
    let rows = catalog::parse("# comment\n\nfirst core 1500 /bin/first one two\nbad core nope /bin/bad\nshort row\nlast fs 20 /bin/last\n");
    assert_eq!(rows.len(), 4);
    let first = rows[0].as_ref().unwrap();
    assert_eq!((&*first.name, &*first.category, first.deadline_ms), ("first", "core", 1500));
    assert_eq!(first.argv, ["/bin/first", "one", "two"]);
    assert_eq!(rows[1].as_ref().unwrap_err().line, 4);
    assert_eq!(rows[2].as_ref().unwrap_err().line, 5);
    assert_eq!(rows[3].as_ref().unwrap().name, "last");
}

#[test]
fn shipped_catalog_loads_24_entries_without_errors() {
    let text = std::fs::read_to_string("userspace/programs/share/bcheck/catalog").unwrap();
    let rows = catalog::parse(&text);
    assert_eq!(rows.len(), 24);
    assert!(rows.iter().all(Result::is_ok), "{rows:?}");
}
