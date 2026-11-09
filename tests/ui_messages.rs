#[path = "../libs/libuitest/src/messages.rs"]
pub mod messages;

#[test]
fn frame_names_check_first_pixel_and_rgb_colors() {
    assert_eq!(messages::pixel("submit", 12, 4, [0x56, 0x34, 0x12, 0], [0xef, 0xcd, 0xab, 255]),
        "pixel-mismatch check=\"submit\" first=12,4 got=#123456 want=#abcdef");
}
#[test]
fn tree_names_node_field_and_both_values() {
    assert_eq!(messages::tree("expect_value(entry)", "entry", "value", &"hello", &"helo"),
        "tree-mismatch check=\"expect_value(entry)\" node=\"entry\" field=value expected=\"hello\" reported=\"helo\"");
}
#[test]
fn input_names_serial_and_disposition_even_without_a_terminal_row() {
    for disposition in ["delivered", "wm-handled", "rejected", "client-rejected", "lost", "missing"] {
        let text = messages::input("delivered", 123456789, disposition, "window=17 reason=stale-layout");
        assert_eq!(text, format!("input check=\"delivered\" serial=123456789 disposition={disposition} window=17 reason=stale-layout"));
    }
}
