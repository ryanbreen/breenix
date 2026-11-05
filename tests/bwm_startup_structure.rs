use std::fs;
use std::path::Path;

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source.find(start).expect(start);
    let tail = &source[start..];
    &tail[..tail.find(end).expect(end)]
}

fn ordered(source: &str, parts: &[&str]) {
    let mut tail = source;
    for part in parts {
        let at = tail.find(part).unwrap_or_else(|| panic!("missing or out of order: {part}"));
        tail = &tail[at + part.len()..];
    }
}

#[test]
fn startup_presents_without_window_receipt_and_finishes_cursor_before_readiness() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let compositor = fs::read_to_string(root.join("userspace/programs/src/bwm/compositor.rs")).unwrap();
    let present = section(&compositor, "pub fn present_startup_desktop", "    /// Query window damage");
    ordered(present, &["self.desktop.wait()", "self.encode(", "gpu::submit(", "gpu::wait(", "gpu::present("]);
    assert!(!present.contains("COMPOSITE"));
    assert!(!present.contains("window::"));
    let chrome = fs::read_to_string(root.join("userspace/programs/src/bwm/chrome.rs")).unwrap();
    let fills = section(&chrome, "pub(super) fn check_desktop_fills", "\n}");
    assert!(fills.contains("present_startup_desktop(None)"));
    let main = fs::read_to_string(root.join("userspace/programs/src/bwm.rs")).unwrap();
    ordered(&main, &["[bwm] Breenix Window Manager starting", "[bwm] GPU compositing mode", "let fills = check_desktop_fills(", "present_startup_desktop(Some(first_pointer))", "gpu.check(0, &[], Some(first_pointer))", "[bwm] first frame read back:", "[bwm] desktop fills the scanout:", "let mut drawn_pointer = first_presented.then_some(first_pointer)", "let mut full_redraw = !first_presented", "Initial window discovery"]);
}
