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


fn until(label: &str, mut ready: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    use breenix_suite::suite::control::{Wait, Watchdog};
    let watchdog = Watchdog::new(Wait::AppOutput)?;
    loop {
        if libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos() >= i128::from(watchdog.deadline_ns()) {
            return Err(format!("{label}: hang watchdog"));
        }
        if ready()? { return Ok(()); }
        libbreenix::time::sleep_ms(10).map_err(|e| e.to_string())?;
    }
}

fn grid(t: &mut App) -> Result<serde_json::Value, String> {
    Ok(t.tree()?.root.value)
}

fn expect_tab(t: &mut App, active: u64, count: u64) -> Result<(), String> {
    until("terminal tab state", || {
        let v = grid(t)?;
        Ok(v["active_tab"] == active && v["tab_count"] == count)
    })
}

fn has_line(v: &serde_json::Value, text: &str) -> bool {
    v["lines"].as_array().is_some_and(|lines| lines.iter().any(|line| line == text))
}

pub fn tabs() -> CaseResult {
    use libuitest::input::Modifier;
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("echo one")?;
    t.expect_text("one")?;
    t.key(20, &[Modifier::Ctrl])?;
    expect_tab(&mut t, 1, 2)?;
    t.expect_text("bsh />")?;
    t.type_line("echo two")?;
    t.expect_text("two")?;
    t.expect_node("grid", |v| !has_line(v, "one"))?;
    t.type_line("/usr/libexec/breenix/vt-script_test --session")?;
    let mut session = None;
    until("second shell session id", || {
        let v = grid(&mut t)?;
        session = v["lines"].as_array().and_then(|lines| lines.iter().filter_map(|v| v.as_str())
            .find_map(|line| line.strip_prefix("SESSION ").and_then(|pid| pid.parse::<i32>().ok())));
        Ok(session.is_some())
    })?;
    let session = session.ok_or("missing second session")?;
    if session <= 1 { return Err("invalid tab session id".into()); }
    libbreenix::signal::kill(-session, 0).map_err(|e| e.to_string())?;
    t.key(17, &[Modifier::Ctrl])?;
    expect_tab(&mut t, 0, 1)?;
    t.expect_text("one")?;
    t.expect_node("grid", |v| !has_line(v, "two"))?;
    until("closed tab group gone and shell reaped", || {
        Ok(matches!(libbreenix::signal::kill(-session, 0),
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ESRCH)))
            && !std::path::Path::new(&format!("/proc/{session}/stat")).exists())
    })?;
    Ok(t.finish()?)
}

pub fn resize() -> CaseResult {
    use libuitest::input::Modifier;
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("echo first-survives")?;
    t.expect_text("first-survives")?;
    let before = grid(&mut t)?;
    t.key(20, &[Modifier::Ctrl])?;
    expect_tab(&mut t, 1, 2)?;
    t.expect_text("bsh />")?;
    t.type_line("echo second-survives")?;
    t.expect_text("second-survives")?;
    // App::resize drags the right and bottom edges with relative mouse motion.
    t.resize(830, 610)?;
    until("resized terminal grid", || {
        let v = grid(&mut t)?;
        Ok(v["cols"].as_u64() > before["cols"].as_u64() && v["rows"].as_u64() > before["rows"].as_u64())
    })?;
    let after = grid(&mut t)?;
    let size = format!("{} {}", after["rows"], after["cols"]);
    t.type_line("/usr/libexec/breenix/vt-script_test --size")?;
    t.expect_text(&size)?;
    t.expect_text("second-survives")?;
    t.key(17, &[Modifier::Ctrl])?;
    expect_tab(&mut t, 0, 1)?;
    t.expect_node("grid", |v| v["cols"] == after["cols"] && v["rows"] == after["rows"])?;
    t.type_line("/usr/libexec/breenix/vt-script_test --size")?;
    t.expect_text(&size)?;
    t.expect_text("first-survives")?;
    Ok(t.finish()?)
}

pub fn scrollback() -> CaseResult {
    use libuitest::input::Modifier;
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.expect_text("bsh />")?;
    let rows = grid(&mut t)?["rows"].as_u64().ok_or("missing rows")?;
    if !(2..=200).contains(&rows) { return Err("unexpected terminal height for scrollback".into()); }
    // Include the final prompt row so page-sized steps can put line 1 exactly at the top.
    let padding = (rows - 201 % rows) % rows;
    t.type_line(&format!("for(let i=1;i<=200;i++)print(i);for(let i=0;i<{padding};i++)print('')"))?;
    until("numbered output complete", || {
        let v = grid(&mut t)?;
        Ok(v["scrollback"].as_u64().is_some_and(|n| n >= 201 - rows)
            && has_line(&v, "bsh />"))
    })?;
    for _ in 0..(1000 / rows + 2) {
        let v = grid(&mut t)?;
        if v["lines"][0] == "1" { return Ok(t.finish()?); }
        t.key(104, &[Modifier::Shift])?;
    }
    Err("scrollback did not show line 1 at the top".into())
}

pub fn paste() -> CaseResult {
    use libuitest::input::Modifier;
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.expect_text("bsh />")?;
    t.type_line("/usr/libexec/breenix/clip-set_test 'echo pasted'")?;
    t.expect_text("CLIPBOARD SET")?;
    t.key(47, &[Modifier::Ctrl, Modifier::Shift])?;
    t.key(28, &[])?;
    t.expect_text("pasted")?;
    Ok(t.finish()?)
}
