fn body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).expect(signature);
    let rest = &source[start..];
    let open = rest.find('{').unwrap();
    let mut depth = 1;
    for (i, c) in rest[open+1..].char_indices() {
        match c { '{' => depth += 1, '}' => depth -= 1, _ => {} }
        if depth == 0 { return &rest[open+1..open+1+i]; }
    }
    panic!("unterminated {signature}")
}
#[test]
fn case_deadline_stays_on_clock_monotonic() {
    let suite = include_str!("../libs/libbreenix/src/suite.rs");
    assert!(body(suite, "fn monotonic_ns(").contains("time::now_monotonic()"));
    assert!(!body(suite, "fn monotonic_ns(").contains("ui_clock"));
    assert!(!suite.contains("ui_clock::now_ns("));
    let time = include_str!("../libs/libbreenix/src/time.rs");
    assert!(body(time, "pub fn now_monotonic(").contains("clock::MONOTONIC"));
}
#[test]
fn configured_breengel_clock_uses_only_page_and_frames_keep_their_render_epoch() {
    let provider = include_str!("../libs/libbreenix/src/ui_clock.rs");
    assert!(body(provider, "pub fn now_ns(").contains("page()"));
    assert!(body(provider, "pub fn wall_ns(").contains("if let Some(p) = page()"));
    assert!(!body(provider, "fn page(").contains("clock_gettime"));
    let facade = include_str!("../libs/breengel/src/clock.rs");
    assert!(facade.contains("pub use libbreenix::ui_clock::*"));
    let window = include_str!("../libs/breengel/src/window.rs");
    assert!(body(window, "pub fn acquire(").contains("crate::clock::stamp()"));
    assert!(body(window, "fn meta(").contains("m.epoch = stamp.epoch"));
    assert!(!body(window, "fn meta(").contains("self.epoch"));
}
