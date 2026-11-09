use breenix_suite::suite::CaseResult;
use libuitest::App;

pub fn command_output() -> CaseResult {
    let mut t = App::launch(&[b"/usr/bin/bterm"], "Terminal")?;
    t.type_line("echo hello")?;
    t.expect_text("hello")?;
    Ok(t.finish()?)
}
