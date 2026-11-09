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
