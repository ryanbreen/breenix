//! Run with `rustc --edition=2021 --test tests/suite_result_records.rs`.
#[path = "../libs/libbreenix/src/suite_result.rs"]
mod result;

use result::{Accounting, MAX_RECORD_BYTES, RECORDS_PER_CASE};

fn frame(check: &str, matched: bool) -> String {
    format!("SUITE fixture FRAME first/one {check} def=0123abcd ref=formula source=window:main commit=1 epoch=2 w=3 h=4 sha256={} result={}{}",
        "a".repeat(64), if matched { "match" } else { "mismatch" },
        if matched { "" } else { " compared=12 differing=1 first=2,3 got=#abcdef want=#123456 artifacts=none:io" })
}

fn accounting<'a>(checks: &'a [&'a str], collect: bool) -> Accounting<'a> {
    Accounting::new("fixture", "first/one", checks, collect).unwrap()
}

#[test]
fn grammar_and_binding_negatives() {
    let good = frame("label", true);
    let bad = frame("label", false);
    let tree = format!("SUITE fixture TREE first/one label window=main commit=1 nodes=2 sha256={}", "b".repeat(64));
    let input = "SUITE fixture INPUT first/one sent=3 processed=1 handled=1 rejected=1 lost=0";
    let collect = "SUITE fixture COLLECT first/one trigger=fail screen=at-failure windows=1 remnants=0 artifacts=complete";
    for line in [&good, &bad, &tree, input, collect] {
        assert!(accounting(&["label"], true).accept(line).is_ok(), "{line}");
    }
    for line in [
        good.replace("FRAME", "FRAMES"), good.replace("fixture", "other"),
        good.replace("first/one", "first/two"), good.replace(" label ", " undeclared "),
        good.replace("0123abcd", "0123abcZ"), good.replace("ref=formula", "ref=unknown"),
        good.replace("source=window:main", "source=other"), good.replace("w=3", "w=0"),
        good.replace("epoch=2", "epoch=-2"), good.replace("commit=1", "commit=18446744073709551616"),
        good.replace("result=match", "result=maybe"), format!("{good} extra=1"),
        good.replace("sha256=aaaa", "sha256=AAAA"), good.replace(" FRAME ", "  FRAME "),
        bad.replace("compared=12", "compared=0"), bad.replace("differing=1", "differing=13"),
        bad.replace("first=2,3", "first=3,3"), bad.replace("got=#abcdef", "got=#abcdeZ"),
        bad.replace("artifacts=none:io", "artifacts=partial:"),
        tree.replace(" label ", " unknown "), tree.replace("nodes=2", "nodes=-1"),
        input.replace("lost=0", "lost=-1"), input.replace("sent=3", "sent=18446744073709551616"),
        collect.replace("trigger=fail", "trigger=pass"), collect.replace("screen=at-failure", "screen=unknown"),
        "SUITE fixture FRAME first/one".into(), format!("{good}\nforged"),
    ] {
        assert!(accounting(&["label"], true).accept(&line).is_err(), "accepted {line}");
    }
    assert!(accounting(&[], false).accept(collect).is_err());
}

#[test]
fn conversion_table_preserves_failures_and_requires_every_match() {
    let checks = ["label", "after"];
    let mut a = accounting(&checks, false);
    assert_eq!(a.problem().as_deref(), Some("frame check `label` never ran"));
    a.accept(&frame("label", true)).unwrap();
    assert_eq!(a.problem().as_deref(), Some("frame check `after` never ran"));
    a.accept(&frame("after", true)).unwrap();
    assert_eq!(a.problem(), None);
    let mut a = accounting(&["label"], false);
    a.accept(&frame("label", false)).unwrap();
    assert_eq!(a.problem().as_deref(), Some("frame check `label` mismatched"));
    assert!(a.accept(&frame("label", true)).is_err());
    assert!(a.problem().is_some());
    let mut a = accounting(&[], false);
    assert_eq!(a.problem(), None);
    assert!(a.accept("garbage").is_err());
    assert!(a.problem().unwrap().contains("malformed"));
}

#[test]
fn duplicates_capacity_and_declarations_fail_closed() {
    assert_eq!(RECORDS_PER_CASE, 256);
    assert_eq!(MAX_RECORD_BYTES, 4096);
    assert!(Accounting::new("fixture", "first/one", &["label", "label"], false).is_err());
    assert!(Accounting::new("fixture", "first/one", &["bad_name"], false).is_err());
    let checks: Vec<String> = (0..257).map(|n| format!("check-{n}")).collect();
    let names: Vec<&str> = checks.iter().map(String::as_str).collect();
    assert!(Accounting::new("fixture", "first/one", &names, false).is_err());
    let mut a = accounting(&names[..256], false);
    for name in &names[..256] { a.accept(&frame(name, true)).unwrap(); }
    assert_eq!(a.problem(), None);
    assert!(a.accept(&frame("check-0", true)).is_err());
    for line in [
        format!("SUITE fixture TREE first/one label window=main commit=1 nodes=1 sha256={}", "a".repeat(64)),
        "SUITE fixture INPUT first/one sent=1 processed=1 handled=0 rejected=0 lost=0 missing=1".into(),
        "SUITE fixture COLLECT first/one trigger=save-all screen=at-collection windows=1 remnants=0 artifacts=complete".into(),
    ] {
        let mut a = accounting(&["label"], true);
        a.accept(&line).unwrap();
        assert!(a.accept(&line).is_err());
    }
}

#[test]
fn stream_drain_keeps_exit_tail_and_bounds_partial_records() {
    let line = frame("label", true);
    let wire = format!("{line}\n");
    for split in 0..wire.len() {
        let mut a = accounting(&["label"], false);
        let mut lines = Vec::new();
        a.feed(&wire.as_bytes()[..split], |s| lines.push(s.to_string()));
        a.feed(&wire.as_bytes()[split..], |s| lines.push(s.to_string()));
        a.eof();
        assert_eq!(lines, [line.clone()]);
        assert_eq!(a.problem(), None);
    }
    for wire in [line.into_bytes(), vec![b'x'; 4097], vec![0xff, b'\n']] {
        let mut a = accounting(&[], false);
        a.feed(&wire, |_| {});
        a.eof();
        assert!(a.problem().is_some());
    }
}

#[test]
fn transport_paths_are_gui_gated_and_helpers_close_the_writer() {
    let source = include_str!("../libs/libbreenix/src/suite.rs");
    assert!(source.contains("if self.gui {\n            for category in self.categories"));
    assert!(source.contains("let mut results = if gui {"));
    assert!(source.contains("run_case(case, limit_ms, self.id, &name, self.gui, &mut screen"));
    assert!(source.contains("ResultPipe::new(suite, name, case.checks, case.collects)"));
    assert!(source.contains("if let Some(results) = results.as_mut() { results.drain(); }"));
    assert!(source.contains("results.finish(outcome)"));
    let spawn = include_str!("../libs/libbreenix/src/suite_spawn.rs");
    assert!(spawn.contains("crate::suite::close_result_output();"));
    assert!(!source.contains("result pipe is optional"));
    assert!(source.contains("io::pipe2(status_flags::O_CLOEXEC).map_err"));
    assert!(source.contains("io::fcntl_setfl(reader, status_flags::O_NONBLOCK)"));
    assert!(!source.contains("io::fcntl_setfl(writer"));
    assert_eq!(source.matches("ResultPipe::new(").count(), 1);
    assert_eq!(source.matches("RESULT_FD.store(").count(), 1);
    assert!(source.contains("results.as_ref().and_then(|r| r.poll_fd())"));
}

fn item<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).unwrap();
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source[brace..].bytes().enumerate() {
        match byte { b'{' => depth += 1, b'}' => depth -= 1, _ => {} }
        if depth == 0 { return &source[start..=brace + offset]; }
    }
    panic!("unclosed {signature}");
}

#[test]
fn runner_conversion_uses_exact_missing_name_and_preserves_primary_failure() {
    let source = include_str!("../libs/libbreenix/src/suite.rs");
    let path = std::env::current_dir().unwrap().join("libs/libbreenix/src/suite_result.rs");
    let code = format!(r#"
#[path = {path:?}] pub mod result;
mod runner {{
use super::result;
fn monotonic_ns() -> Option<i128> {{ Some(0) }}
const SWEEP_MS: u64 = 2000;
mod io {{
    pub mod poll_events {{ pub const POLLIN: i16 = 1; }}
    pub struct PollFd;
    impl PollFd {{ pub fn new(_: i32, _: i16) -> Self {{ Self }} }}
    pub fn poll(_: &mut [PollFd], _: i32) -> Result<usize, ()> {{ panic!("conversion fixture already reached EOF") }}
}}
{}
const MSG_MAX: usize = 160;
{}
{}
struct ResultPipe<'a> {{ accounting: result::Accounting<'a>, eof: bool, reader: i32 }}
impl ResultPipe<'_> {{
    // This check drives the runner's conversion after its EOF drain, without I/O.
    fn drain_reads(&mut self, _reads: usize) {{ self.eof = true; }}
    {}
}}
#[test] fn conversions() {{
    for verdict in 0..3 {{
        let mut pipe = ResultPipe {{ accounting: result::Accounting::new("fixture", "first/one", &["label"], false).unwrap(), eof: false, reader: 3 }};
        let prior = match verdict {{ 0 => Outcome::Pass {{ms: 17}}, 1 => Outcome::Fail {{ms: 17, msg: "primary fault".into()}}, _ => Outcome::Skip {{msg: "skip fault".into()}} }};
        match pipe.finish(prior) {{
            Outcome::Fail {{ms, msg}} => {{
                assert_eq!(ms, if verdict == 2 {{0}} else {{17}});
                assert!(msg.contains("frame check `label` never ran"));
                if verdict == 0 {{ assert_eq!(msg, "frame check `label` never ran"); }}
                if verdict == 1 {{ assert!(msg.starts_with("primary fault;")); }}
                if verdict == 2 {{ assert!(msg.contains("skip fault")); }}
            }},
            _ => panic!("missing record did not fail the case"),
        }}
    }}
}}
}}
"#, item(source, "pub(super) enum Outcome"), item(source, "fn plain("),
        item(source, "fn with_note("), item(source, "fn finish(&mut self, outcome:"));
    let dir = std::env::temp_dir().join(format!("e22-result-conversion-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("check.rs"), code).unwrap();
    let build = std::process::Command::new("rustc").args(["--edition=2021", "-Dwarnings", "--test"])
        .arg(dir.join("check.rs")).arg("-o").arg(dir.join("check")).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = std::process::Command::new(dir.join("check")).output().unwrap();
    assert!(run.status.success(), "{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn runner_waits_for_post_reap_eof_and_refuses_a_retained_writer() {
    let source = include_str!("../libs/libbreenix/src/suite.rs");
    let path = std::env::current_dir().unwrap().join("libs/libbreenix/src/suite_result.rs");
    let code = format!(r#"
#[path = {path:?}] pub mod result;
pub mod runner {{
use super::result;
use std::sync::atomic::{{AtomicU64, Ordering}};
pub static NOW: AtomicU64 = AtomicU64::new(0);
pub fn monotonic_ns() -> Option<i128> {{ Some(i128::from(NOW.load(Ordering::Relaxed)) * 1_000_000) }}
pub const SWEEP_MS: u64 = 2000;
pub mod io {{
    pub mod poll_events {{ pub const POLLIN: i16 = 1; }}
    pub struct PollFd;
    impl PollFd {{ pub fn new(_: i32, _: i16) -> Self {{ Self }} }}
    pub fn poll(_: &mut [PollFd], timeout: i32) -> Result<usize, ()> {{
        super::NOW.fetch_add(timeout as u64, super::Ordering::Relaxed);
        Ok(0)
    }}
}}
{}
const MSG_MAX: usize = 160;
{}
{}
struct ResultPipe<'a> {{ accounting: result::Accounting<'a>, eof: bool, reader: i32, close_at: u64 }}
impl ResultPipe<'_> {{
    fn drain_reads(&mut self, _reads: usize) {{
        self.eof = NOW.load(Ordering::Relaxed) >= self.close_at;
        if self.eof {{ self.accounting.eof(); }}
    }}
    {}
}}
#[test] fn post_reap_close() {{
    let _ = Outcome::Skip {{ msg: "unused alternative".into() }};
    for close_at in [1, 2, 17, 100, u64::MAX] {{
        NOW.store(0, Ordering::Relaxed);
        let mut pipe = ResultPipe {{ accounting: result::Accounting::new("fixture", "first/one", &[], false).unwrap(),
                                    eof: false, reader: 3, close_at }};
        // Also use the mocked poll surface on the old implementation: no dead-code suppression.
        let _ = io::poll(&mut [io::PollFd::new(pipe.reader, io::poll_events::POLLIN)], 0);
        let _ = monotonic_ns();
        match pipe.finish(Outcome::Pass {{ms: 7}}) {{
            Outcome::Pass {{ms}} if close_at != u64::MAX => assert_eq!(ms, 7),
            Outcome::Fail {{msg, ..}} if close_at == u64::MAX =>
                assert!(msg.contains("result pipe did not reach EOF after case teardown")),
            _ => panic!("post-reap writer close at {{close_at}} gave the wrong verdict"),
        }}
        assert!(NOW.load(Ordering::Relaxed) <= SWEEP_MS, "EOF wait exceeded its bound");
        if close_at == u64::MAX {{ assert_eq!(NOW.load(Ordering::Relaxed), SWEEP_MS); }}
    }}
}}
}}
"#, item(source, "pub(super) enum Outcome"), item(source, "fn plain("),
        item(source, "fn with_note("), item(source, "fn finish(&mut self, outcome:"));
    let dir = std::env::temp_dir().join(format!("e22-result-eof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("check.rs"), code).unwrap();
    let build = std::process::Command::new("rustc").args(["--edition=2021", "-Dwarnings", "--test"])
        .arg(dir.join("check.rs")).arg("-o").arg(dir.join("check")).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = std::process::Command::new(dir.join("check")).output().unwrap();
    assert!(run.status.success(), "{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    std::fs::remove_dir_all(dir).unwrap();
}
