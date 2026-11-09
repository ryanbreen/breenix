use breenix_suite::suite::CaseResult;
use libuitest::App;

pub fn command_output() -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("echo hello")?;
    t.expect_text("hello")?;
    Ok(t.finish()?)
}

pub fn prompt() -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.expect_text("bsh />")?;
    Ok(t.finish()?)
}

pub fn exit_status() -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("false; echo $?")?;
    t.expect_text("1")?;
    Ok(t.finish()?)
}

pub fn ctrl_c() -> CaseResult {
    use libuitest::input::Modifier;
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.expect_text("bsh />")?;
    t.type_line("cat")?;
    t.type_line("interrupt-ready")?;
    // Wait for both the line discipline's echo and cat's copy before interrupting.
    let start = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
    loop {
        if t.expect_node("grid", |v| v["lines"].as_array().is_some_and(|lines|
            lines.iter().filter(|line| line.as_str() == Some("interrupt-ready")).count() >= 2)).is_ok() { break; }
        if libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos().saturating_sub(start) >= 10_000_000_000 {
            return Err("cat did not echo its input".into());
        }
        libbreenix::time::sleep_ms(10).map_err(|e| e.to_string())?;
    }
    t.key(46, &[Modifier::Ctrl])?;
    t.type_line("echo interrupt-done")?;
    t.expect_text("interrupt-done")?;
    Ok(t.finish()?)
}

pub fn exec_tab() -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm", b"-e", b"/bin/echo", b"exec-output"], "Terminal")?;
    t.expect_text("exec-output")?;
    t.expect_text("[exited 0]")?;
    Ok(t.finish()?)
}
