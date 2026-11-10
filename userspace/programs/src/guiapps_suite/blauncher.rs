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
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-filter"], "\u{1}Launcher")?;
    t.type_text("lOg")?;
    t.wait_node("apps", |v| v["query"] == "lOg" && v["rows"].as_array().is_some_and(|r| r.len() == 2 && r[0]["name"] == "Alpha" && r[1]["name"] == "Logbook"))?;
    Ok(t.finish()?)
}

pub fn no_results() -> CaseResult {
    std::fs::write("/tmp/blauncher-empty", "Alpha\tFirst\tgui\t/bin/a\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-empty"], "\u{1}Launcher")?;
    t.type_text("missing")?;
    t.wait_node("apps", |v| v["rows"] == serde_json::json!([]) && v["empty"] == "No matches")?;
    Ok(t.finish()?)
}

pub fn keys_select() -> CaseResult {
    std::fs::write("/tmp/blauncher-keys", "Alpha\tFirst\tgui\t/bin/a\nBeta\tSecond\tgui\t/bin/b\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-keys"], "\u{1}Launcher")?;
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
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-gui"], "\u{1}Launcher")?;
    t.key_exit(28, 0)?;
    t.attach("Ack")?;
    t.wait_node("ack", |v| v["argv"] == expected)?;
    let records = std::fs::read_to_string("/tmp/ack-app.jsonl").map_err(|e| e.to_string())?;
    let lines: Vec<_> = records.lines().collect();
    if lines.len() != 1 { return Err(format!("expected one acknowledgment: {records:?}").into()); }
    let record: serde_json::Value = serde_json::from_str(lines[0]).map_err(|e| e.to_string())?;
    if record["argv"] != expected {
        return Err(format!("unexpected acknowledgment: {record}").into());
    }
    Ok(t.finish()?)
}

pub fn launch_terminal() -> CaseResult {
    std::fs::write("/tmp/blauncher-terminal", "Echo\tFixture\tterminal\t/bin/echo\tlaunched-terminal\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-terminal"], "\u{1}Launcher")?;
    t.key_exit(28, 0)?;
    t.attach("Terminal")?;
    t.expect_text("launched-terminal")?;
    t.expect_text("[exited 0]")?;
    Ok(t.finish()?)
}

pub fn launch_missing() -> CaseResult {
    std::fs::write("/tmp/blauncher-missing", "Missing\tFixture\tgui\t/usr/bin/no-such-app\n").map_err(|e| e.to_string())?;
    let mut t = App::launch(&[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-missing"], "\u{1}Launcher")?;
    let launcher = std::fs::read_dir("/proc").map_err(|e| e.to_string())?
        .filter_map(Result::ok).filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u64>().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            if stat.contains("(blauncher)") { Some(pid) } else { None }
        }).collect::<Vec<_>>();
    if launcher.len() != 1 { return Err(format!("expected one launcher, found {launcher:?}").into()); }
    t.key(28, &[])?;
    t.wait_node("apps", |v| v["error"] == "Missing program: /usr/bin/no-such-app")?;
    t.attach("\u{1}Launcher")?;
    for entry in std::fs::read_dir("/proc").map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_str().and_then(|s| s.parse::<u64>().ok()).is_none() { continue; }
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string().into()),
        };
        let parent = stat.rsplit_once(") ").and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u64>().ok()).ok_or("invalid proc stat")?;
        if parent == launcher[0] { return Err(format!("launcher child remains: {stat}").into()); }
    }
    Ok(t.finish()?)
}

pub fn close() -> CaseResult {
    std::fs::write("/tmp/blauncher-close", "Alpha\tFirst\tgui\t/bin/a\n").map_err(|e| e.to_string())?;
    let argv: &[&[u8]] = &[b"/usr/bin/blauncher", b"--catalog", b"/tmp/blauncher-close"];
    let mut t = App::launch(argv, "\u{1}Launcher")?;
    t.key_exit(1, 0)?;
    t.finish()?;
    let mut t = App::launch(argv, "\u{1}Launcher")?;
    t.click_exit(0, 0, 0)?;
    Ok(t.finish()?)
}
