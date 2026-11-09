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
    t.expect_text("interrupt-ready")?;
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
