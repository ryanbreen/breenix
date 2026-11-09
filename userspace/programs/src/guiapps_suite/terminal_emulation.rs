use breenix_suite::suite::CaseResult;
use libuitest::App;
use serde_json::json;

fn run(name: &str, lines: [&str; 4], red: bool, cursor: [usize; 2]) -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line(&format!("/usr/libexec/breenix/vt-script_test {name}"))?;
    t.expect_text(lines[0])?;
    let start = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
    loop {
    let result = t.expect_node("grid", |v| {
        let expected_runs = if red { json!([{"start":6,"end":9,"fg":[205,49,49],"bg":[30,30,40],"bold":false,"underline":false,"inverse":false}]) } else { json!([]) };
        v["cursor"] == json!(cursor) && v["alt"] == false && v["scrollback"] == 0 && v["offset"] == 0
            && (0..4).all(|y| v["lines"][y] == lines[y] && v["runs"][y] == if y == 0 { expected_runs.clone() } else { json!([]) })
            && v["lines"].as_array().is_some_and(|rows| rows.iter().skip(4).all(|row| row == ""))
            && v["runs"].as_array().is_some_and(|rows| rows.iter().skip(4).all(|row| row == &json!([])))
    });
    if result.is_ok() { break; }
    if libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos().saturating_sub(start) >= 10_000_000_000 { return Err(result.unwrap_err().into()); }
    libbreenix::time::sleep_ms(10).map_err(|e| e.to_string())?;
    }
    Ok(t.finish()?)
}
pub fn vt_screen() -> CaseResult { run("vt-screen", ["Hello red!", "moved", "", ""], true, [5, 1]) }
pub fn scroll_region() -> CaseResult { run("scroll-region", ["top", "two", "three", "bottom"], false, [0, 4]) }
pub fn alt_screen() -> CaseResult { run("alt-screen", ["main restored", "", "", ""], false, [13, 0]) }
pub fn utf8() -> CaseResult { run("utf8", ["Aé界�B", "", "", ""], false, [5, 0]) }

pub fn keys() -> CaseResult {
    use libuitest::input::Modifier;
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("/usr/libexec/breenix/vt-script_test --keys")?;
    t.expect_text("KEYS READY")?;
    for (key, expected) in [
        (103, "1b5b41"), (108, "1b5b42"), (106, "1b5b43"), (105, "1b5b44"),
        (102, "1b5b48"), (107, "1b5b46"), (111, "1b5b337e"), (104, "1b5b357e"),
        (109, "1b5b367e"), (59, "1b4f50"), (60, "1b4f51"), (61, "1b4f52"),
        (62, "1b4f53"), (14, "7f"),
    ] {
        t.key(key, &[])?;
        t.expect_text(expected)?;
    }
    t.key(30, &[Modifier::Ctrl])?;
    t.expect_text("01")?;
    t.expect_text("APPLICATION READY")?;
    for (key, expected) in [(103, "1b4f41"), (108, "1b4f42"), (106, "1b4f43"),
        (105, "1b4f44"), (102, "1b4f48"), (107, "1b4f46")] {
        t.key(key, &[])?;
        t.expect_text(expected)?;
    }
    Ok(t.finish()?)
}
