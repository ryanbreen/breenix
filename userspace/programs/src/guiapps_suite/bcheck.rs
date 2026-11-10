use breenix_suite::suite::CaseResult;
use libuitest::App;
use std::cell::Cell;

const FIXTURE: &str = "/usr/libexec/breenix/bcheck-fixture_test";

fn launch(name: &str, catalog: &str) -> Result<App, String> {
    let path = format!("/tmp/bcheck-{name}.catalog");
    std::fs::write(&path, catalog).map_err(|e| e.to_string())?;
    App::launch(&[b"/usr/bin/bcheck", b"--catalog", path.as_bytes()], "Self-Check")
}

pub fn results_truthful() -> CaseResult {
    let catalog = format!("pass test 5000 {FIXTURE} pass\nexit1 test 5000 {FIXTURE} exit 1\nsignal test 5000 {FIXTURE} signal\nmissing test 5000 /missing-bcheck-fixture\nexit127 test 5000 {FIXTURE} exit 127\n");
    let mut t = launch("results", &catalog)?;
    t.wait_node("rows", |v| {
        let Some(rows) = v["rows"].as_array() else { return false };
        rows.len() == 5 && rows.iter().all(|r| r["state"] == "done")
            && rows[0]["name"] == "pass" && rows[0]["verdict"] == "pass"
            && rows[1]["name"] == "exit1" && rows[1]["verdict"] == "exit 1"
            && rows[2]["name"] == "signal" && rows[2]["verdict"] == "SIGTERM"
            && rows[3]["name"] == "missing" && rows[3]["verdict"].as_str().is_some_and(|s| s.starts_with("exec failed:"))
            && rows[4]["name"] == "exit127" && rows[4]["verdict"] == "exit 127"
    })?;
    t.wait_node("totals", |v| v["passed"] == 1 && v["failed"] == 4 && v["done"] == 5 && v["pending"] == 0)?;
    t.key_exit(1, 1)?;
    Ok(t.finish()?)
}

pub fn hang() -> CaseResult {
    let mut t = launch("hang", &format!("hang test 1000 {FIXTURE} hang\n"))?;
    let pid = Cell::new(0);
    t.wait_node("rows", |v| {
        let row = &v["rows"][0];
        if row["state"] == "running" { pid.set(row["pid"].as_u64().unwrap_or(0)); }
        pid.get() != 0
    })?;
    t.wait_node("rows", |v| v["rows"][0]["state"] == "done" && v["rows"][0]["verdict"] == "timed out")?;
    t.wait_node("totals", |v| v["passed"] == 0 && v["failed"] == 1 && v["done"] == 1 && v["pending"] == 0)?;
    let path = format!("/proc/{}/stat", pid.get());
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        other => return Err(format!("timed-out child still present at {path}: {other:?}").into()),
    }
    t.key_exit(1, 1)?;
    Ok(t.finish()?)
}

pub fn forged_output() -> CaseResult {
    let mut t = launch("forged", &format!("forged test 5000 {FIXTURE} forged\n"))?;
    t.wait_node("rows", |v| v["rows"][0]["name"] == "forged" && v["rows"][0]["state"] == "done" && v["rows"][0]["verdict"] == "exit 1")?;
    t.wait_node("totals", |v| v["passed"] == 0 && v["failed"] == 1 && v["done"] == 1 && v["pending"] == 0)?;
    t.key_exit(1, 1)?;
    Ok(t.finish()?)
}

pub fn totals_exit() -> CaseResult {
    let mut t = launch("totals", &format!("pass test 5000 {FIXTURE} pass\nfail test 5000 {FIXTURE} exit 1\n"))?;
    t.wait_node("rows", |v| v["rows"].as_array().is_some_and(|r| r.len() == 2 && r[0]["verdict"] == "pass" && r[1]["verdict"] == "exit 1" && r.iter().all(|row| row["state"] == "done")))?;
    t.wait_node("totals", |v| v["passed"] == 1 && v["failed"] == 1 && v["done"] == 2 && v["pending"] == 0)?;
    t.key_exit(1, 1)?;
    Ok(t.finish()?)
}
