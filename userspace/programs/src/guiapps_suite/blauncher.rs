use breenix_suite::suite::CaseResult;
use libuitest::App;

pub fn open() -> CaseResult {
    Err("not built (E23 23.2)".into())
}

pub fn filter() -> CaseResult {
    std::fs::write("/tmp/blauncher-filter", "Alpha\tSystem LOG viewer\tgui\t/bin/a\nLogbook\tOther\tterminal\t/bin/b\nThird\tElse\tgui\t/bin/c\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-filter"], "Launcher")?;
    t.type_text("lOg")?;
    t.wait_node("apps", |v| v["query"] == "lOg" && v["rows"].as_array().is_some_and(|r| r.len() == 2 && r[0]["name"] == "Alpha" && r[1]["name"] == "Logbook"))?;
    Ok(t.finish()?)
}

pub fn no_results() -> CaseResult {
    std::fs::write("/tmp/blauncher-empty", "Alpha\tFirst\tgui\t/bin/a\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-empty"], "Launcher")?;
    t.type_text("missing")?;
    t.wait_node("apps", |v| v["rows"] == serde_json::json!([]) && v["empty"] == "No matches")?;
    Ok(t.finish()?)
}

pub fn keys_select() -> CaseResult {
    std::fs::write("/tmp/blauncher-keys", "Alpha\tFirst\tgui\t/bin/a\nBeta\tSecond\tgui\t/bin/b\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-keys"], "Launcher")?;
    t.key(103, &[])?;
    t.wait_node("apps", |v| v["selected"] == 0)?;
    t.key(108, &[])?;
    t.wait_node("apps", |v| v["selected"] == 1)?;
    t.key(108, &[])?;
    t.wait_node("apps", |v| v["selected"] == 1)?;
    t.key(103, &[])?;
    t.wait_node("apps", |v| v["selected"] == 0)?;
    Ok(t.finish()?)
}

pub fn launch_gui() -> CaseResult {
    Err("not built (E23 23.2)".into())
}

pub fn launch_terminal() -> CaseResult {
    Err("not built (E23 23.2)".into())
}

pub fn launch_missing() -> CaseResult {
    Err("not built (E23 23.2)".into())
}

pub fn close() -> CaseResult {
    Err("not built (E23 23.2)".into())
}
