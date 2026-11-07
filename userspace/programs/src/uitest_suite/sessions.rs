//! Session core checks; helpers run through the real sibling supervisor.
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{process, signal};
use libuitest::{Session, ScenarioSpec, ScenarioOutcome, FailureClass, run_scenario};

const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";
const FIXTURE: &[u8] = b"/usr/libexec/breenix/ui-runner-fixture_test";

pub fn launch_argv() -> CaseResult {
    let mut session = Session::connect()?;
    let argv: &[&[u8]] = &[APP, b"title with spaces\n=\\\"", b"--hang"];
    let env: &[&[u8]] = &[b"EMPTY=", b"VALUE=space newline\n=\\\"", b"TZ=UTC"];
    let pid = session.launch(argv, env)?;
    let echo = session.read_line(pid)?;
    // Independent literal wire expectation; do not call the app's echo encoder.
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let expected = format!("UITEST-ECHO 3 {} {} {} 3 {} {} {}", hex(APP), hex(argv[1]), hex(argv[2]), hex(env[0]), hex(env[1]), hex(env[2]));
    check(echo == expected, &format!("argv/env echo differs: {echo:?}"))?;
    session.teardown()?;
    check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "echo app survived teardown")
}

pub fn teardown_hung() -> CaseResult {
    let mut session = Session::connect()?;
    let pid = session.launch(&[APP, b"hung", b"--hang"], &[])?;
    session.read_line(pid)?;
    check(signal::kill(pid as i32, 0).is_ok(), "hung app not alive before teardown")?;
    let exits = session.teardown()?;
    check(exits.iter().any(|e| e.pid == pid && process::wifsignaled(e.status) && process::wtermsig(e.status) == signal::SIGKILL), "hung app lacks reaped SIGKILL status")?;
    check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "hung app not reaped")
}

pub fn app_crash() -> CaseResult {
    let mut scenario = |_: &mut libuitest::RecordClient<'_>| {
        let result = (|| -> Result<(), String> {
            let mut session = Session::connect()?;
            let pid = session.launch(&[APP, b"crashing", b"--crash=11"], &[])?;
            session.wait(pid)?;
            Ok(())
        })();
        match result {
            Err(message) if message.contains("uitest-app_test") && message.contains("SIGSEGV") =>
                ScenarioOutcome::fail(FailureClass::AppCrashed, "launch", &message),
            Err(message) => ScenarioOutcome::fail(FailureClass::Infrastructure, "launch", &message),
            Ok(()) => ScenarioOutcome::pass(),
        }
    };
    let result = run_scenario(ScenarioSpec { suite: "ui-testing", case: "sessions/app-crash", checks: &[], limit_ms: 3000, run: &mut scenario });
    check(matches!(&result, ScenarioOutcome::Fail { class: FailureClass::AppCrashed, evidence, .. } if evidence.contains("uitest-app_test") && evidence.contains("SIGSEGV")), &format!("wrong crash outcome: {result:?}"))
}

pub fn orphan(session: &mut Session) -> Result<u64, String> {
    let supervisor = session.supervisor_pid().to_string();
    let app = session.launch(&[FIXTURE, b"--orphan-helper", supervisor.as_bytes()], &[])?;
    let line = session.read_line(app)?;
    let parts: Vec<_> = line.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "ORPHAN" || parts[2] != supervisor { return Err(format!("bad adoption witness: {line}")); }
    let pid: u64 = parts[1].parse().map_err(|_| "bad orphan pid")?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    let parent = stat.rsplit_once(") ").and_then(|(_, s)| s.split_whitespace().nth(1));
    if parent != Some(supervisor.as_str()) { return Err("orphan parent is not supervisor".into()); }
    Ok(pid)
}

pub(crate) fn reaped_orphan() -> CaseResult {
    let mut session = Session::connect()?;
    let pid = orphan(&mut session)?;
    // A release file gives the orphan a deterministic exit after the ppid assertion.
    std::fs::write(format!("/tmp/uitest-orphan-{pid}"), b"exit").map_err(|e| e.to_string())?;
    let exit = session.wait_exit(pid)?;
    check(process::wifexited(exit.status) && process::wexitstatus(exit.status) == 7, "orphan status is not 7")?;
    check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "orphan was not reaped")?;
    std::fs::remove_file(format!("/tmp/uitest-orphan-{pid}")).map_err(|e| e.to_string())?;
    session.teardown()?;
    Ok(())
}

pub fn subreaper() -> CaseResult {
    reaped_orphan()?;
    let mut scenario = |_: &mut libuitest::RecordClient<'_>| match reaped_orphan() {
        Ok(()) => ScenarioOutcome::pass(),
        Err(error) => ScenarioOutcome::fail(FailureClass::Infrastructure, "orphan", &format!("{error:?}")),
    };
    let result = run_scenario(ScenarioSpec { suite: "ui-testing", case: "runner/subreaper", checks: &[], limit_ms: 3000, run: &mut scenario });
    check(result == ScenarioOutcome::pass(), &format!("inner orphan: {result:?}"))?;
    for mode in ["subreaper", "supervisor-died"] {
        let (text, exit) = super::runner::fixture(mode)?;
        let failed = mode == "supervisor-died";
        check(exit == i32::from(failed), "fixture exit differs")?;
        let expected = format!("CASE fixture/{mode} {}", if failed { "FAIL" } else { "PASS" });
        check(text.contains(&expected), "fixture case verdict differs")?;
        check(text.contains(&format!("DONE passed={} failed={} skipped=0 total=1", usize::from(!failed), usize::from(failed))), "fixture DONE differs")?;
        if failed {
            check(text.contains("artifacts=none:supervisor-died") && text.contains("session supervisor died"), "missing dead supervisor failure")?;
            let prefix = "SUITE runner-fixture VALUE fixture/supervisor-died orphan-pid=";
            let pid = text.lines().find_map(|l| l.strip_prefix(prefix)).and_then(|s| s.strip_suffix("pid")).and_then(|s| s.parse::<u64>().ok()).ok_or("missing orphan pid")?;
            check(!std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "runner did not reap orphan")?;
        }
    }
    Ok(())
}
