use breenix_suite::suite::CaseResult;
use libuitest::{input::{Keyboard, LivePointer}, tree::{self, Query, Role}, Session};

pub fn command_output() -> CaseResult {
    let mut session = Session::desktop()?;
    let pid = session.launch_app(&[b"/bin/bterm"], 0)?;
    let result: Result<(), String> = (|| {
        let id = session.wait_window("Terminal")?.id;
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        let mut keys = Keyboard::new(live);
        keys.type_text(id, "echo hello")?;
        keys.chord(id, &[], 28)?;
        let live = keys.into_backend();
        let start = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos();
        loop {
            let tree = live.tree(id)?;
            let node = tree::find(&tree, &Query { key: Some("grid"), role: Some(Role::Terminal), ..Query::default() })?;
            let lines = node.value["lines"].as_array().ok_or("terminal lines missing")?;
            if lines.iter().any(|line| line.as_str() == Some("hello")) { break Ok(()); }
            if libbreenix::time::now_monotonic().map_err(|e| e.to_string())?.as_nanos() - start >= 10_000_000_000 {
                break Err(format!("terminal output timed out: {lines:?}"));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    })();
    let exits = session.teardown()?;
    let exit = exits.iter().find(|exit| exit.pid == pid).ok_or("app exit not witnessed during teardown")?;
    if !libbreenix::process::wifsignaled(exit.status) || libbreenix::process::wtermsig(exit.status) != libbreenix::signal::SIGKILL {
        return Err(format!("app exited before teardown status={}", exit.status).into());
    }
    result?;
    Ok(())
}
