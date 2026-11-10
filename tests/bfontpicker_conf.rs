#[path = "../userspace/programs/src/bfontpicker/conf.rs"]
mod conf;

#[test]
fn changes_only_the_requested_value() {
    let input = "# heading\r\n\r\n  mono.font = old.ttf  \r\nunknown = keep # comment\nmono.size=12\n# mono.font=comment\n";
    assert_eq!(conf::set(input, "mono.font", "new.ttf"), "# heading\r\n\r\n  mono.font = new.ttf  \r\nunknown = keep # comment\nmono.size=12\n# mono.font=comment\n");
}

#[test]
fn adds_missing_key_without_losing_existing_bytes() {
    assert_eq!(conf::set("# comment\nunknown=yes", "display.font", "a.ttf"), "# comment\nunknown=yes\ndisplay.font=a.ttf\n");
    assert_eq!(conf::set("", "mono.font", "a.ttf"), "mono.font=a.ttf\n");
}
