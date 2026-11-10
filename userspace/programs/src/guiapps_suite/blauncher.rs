use breenix_suite::suite::CaseResult;
use libuitest::App;

pub fn open() -> CaseResult {
    let catalog = std::fs::read_to_string("/usr/share/applications/catalog").map_err(|e| e.to_string())?;
    let rows: Vec<_> = catalog.lines().filter(|line| !line.is_empty() && !line.starts_with('#')).take(12)
        .map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next().ok_or("catalog name missing")?;
            let description = fields.next().ok_or("catalog description missing")?;
            let mode = fields.next().ok_or("catalog mode missing")?;
            Ok(serde_json::json!({"name": name, "description": description, "mode": mode}))
        }).collect::<Result<_, &str>>()?;
    if rows.is_empty() { return Err("shipped catalog is empty".into()); }
    let mut t = App::hotkey(&[libbreenix::input::key::LEFT_SUPER, libbreenix::input::key::LEFT_SUPER], "\u{1}Launcher")?;
    t.wait_node("apps", |v| v["focused"] == "query" && v["query"] == "" && v["rows"].as_array() == Some(&rows))?;
    Ok(t.finish()?)
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
    const ACK: &str = "/usr/libexec/breenix/ack-app_test";
    let nonce = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos());
    let expected = serde_json::json!([ACK, "--window", "Ack", "a", "b c", "--nonce", nonce]);
    let _ = std::fs::remove_file("/tmp/ack-app.jsonl");
    std::fs::write("/tmp/blauncher-gui", format!("Ack\tFixture\tgui\t{ACK}\t--window\tAck\ta\tb c\t--nonce\t{nonce}\n")).map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-gui"], "Launcher")?;
    t.key(28, &[])?;
    t.wait_exit(0)?;
    t.attach("Ack")?;
    t.wait_node("ack", |v| v["argv"] == expected)?;
    let records = std::fs::read_to_string("/tmp/ack-app.jsonl").map_err(|e| e.to_string())?;
    let lines: Vec<_> = records.lines().collect();
    if lines.len() != 1 { return Err(format!("expected one acknowledgment: {records:?}")); }
    let record: serde_json::Value = serde_json::from_str(lines[0]).map_err(|e| e.to_string())?;
    if record["argv"] != expected {
        return Err(format!("unexpected acknowledgment: {record}"));
    }
    Ok(t.finish()?)
}

pub fn launch_terminal() -> CaseResult {
    Err("not built (E23 23.2)".into())
}

pub fn launch_missing() -> CaseResult {
    Err("not built (E23 23.2)".into())
}

pub fn close() -> CaseResult {
    std::fs::write("/tmp/blauncher-close", "Alpha\tFirst\tgui\t/bin/a\n").map_err(|e| e.to_string())?;
    let argv: &[&[u8]] = &[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-close"];
    let mut t = App::launch(argv, "Launcher")?;
    t.key(1, &[])?;
    t.wait_exit(0)?;
    t.finish()?;
    let mut t = App::launch(argv, "Launcher")?;
    t.click(0, 0)?;
    t.wait_exit(0)?;
    Ok(t.finish()?)
}
