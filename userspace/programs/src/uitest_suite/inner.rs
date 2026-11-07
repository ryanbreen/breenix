//! Inner-executor checks; the outer table declares no inner record obligations.
use breenix_suite::suite::{check, spawn, CaseResult, Run};
use libbreenix::{process, signal, time};
use libuitest::{classify, run_scenario, Classification, Expected, FailureClass, RecordClient, ScenarioOutcome, ScenarioSpec};

fn run(checks: &[&str], body: &mut dyn FnMut(&mut RecordClient<'_>) -> ScenarioOutcome) -> ScenarioOutcome {
    run_scenario(ScenarioSpec { suite: "inner", case: "scenario/one", checks, limit_ms: 1000, run: body })
}

pub fn inner_pass() -> CaseResult {
    let outer = process::getpid()?;
    let result = run(&[], &mut |_| {
        if process::getpid().is_ok_and(|pid| pid != outer) && matches!(process::get_child_subreaper(), Ok(true)) {
            ScenarioOutcome::pass()
        } else { ScenarioOutcome::fail(FailureClass::Infrastructure, "subreaper", "wrong process or flag") }
    });
    check(classify(&Expected::Pass, &result) == Classification::CleanPass, &format!("clean scenario: {result:?}"))
}

pub fn inner_wrong_class() -> CaseResult {
    let expected = Expected::Fault { class: FailureClass::PixelMismatch, operation: "label", evidence: "first=12,4" };
    let clean = run(&[], &mut |_| ScenarioOutcome::pass());
    check(classify(&Expected::Pass, &clean).accepted(), "positive control failed")?;
    let wrong = run(&[], &mut |_| ScenarioOutcome::fail(FailureClass::TreeMismatch, "label", "first=12,4"));
    check(classify(&expected, &wrong) == Classification::WrongClass, "wrong class accepted as pixel detection")
}

pub fn inner_crash_not_detection() -> CaseResult {
    let clean = run(&[], &mut |_| ScenarioOutcome::pass());
    check(classify(&Expected::Pass, &clean).accepted(), "positive control failed")?;
    let result = run(&[], &mut |_| {
        let mut crash = || {
            let _ = signal::kill(process::getpid().unwrap().raw() as i32, signal::SIGSEGV);
            97
        };
        let pid = match spawn(&[], Run::Call(&mut crash)) {
            Ok(pid) => pid,
            Err(error) => return ScenarioOutcome::fail(FailureClass::Infrastructure, "launch(app)", &format!("{error:?}")),
        };
        let mut status = 0;
        if process::waitpid(pid.raw() as i32, &mut status, 0).is_ok()
            && process::wifsignaled(status) && process::wtermsig(status) == signal::SIGSEGV {
            ScenarioOutcome::fail(FailureClass::AppCrashed, "app", "signal=11")
        } else { ScenarioOutcome::fail(FailureClass::Infrastructure, "wait(app)", "no crash witness") }
    });
    let expected = Expected::Fault { class: FailureClass::PixelMismatch, operation: "label", evidence: "first=12,4" };
    check(classify(&expected, &result) == Classification::Crash, "app crash accepted as pixel detection")?;
    check(matches!(result, ScenarioOutcome::Fail { ref operation, ref evidence, .. } if operation == "app" && evidence == "signal=11"), "crash witness missing")?;
    // A dying scenario logic process must also be converted by the executor, not decoded as Pass.
    let died = run(&[], &mut |_| {
        let _ = signal::kill(process::getpid().unwrap().raw() as i32, signal::SIGSEGV);
        ScenarioOutcome::pass()
    });
    check(classify(&expected, &died) == Classification::Crash, "scenario death accepted as detection")
}

pub fn inner_teardown_scoped() -> CaseResult {
    let mut hold = || { loop { let _ = time::sleep_ms(100); } };
    let outside = spawn(&[], Run::Call(&mut hold))?;
    let result = run(&[], &mut |_| {
        let mut nested = || {
            loop { let _ = time::sleep_ms(100); }
        };
        let direct = match spawn(&[], Run::Call(&mut nested)) {
            Ok(pid) => pid,
            Err(error) => return ScenarioOutcome::fail(FailureClass::Infrastructure, "spawn(hung)", &format!("{error:?}")),
        };
        // An orphan outlives its short-lived parent and is adopted by the scenario subreaper.
        let mut fork_orphan = || {
            match spawn(&[], Run::Call(&mut nested)) {
                Ok(pid) => pid.raw() as i32,
                Err(_) => 0,
            }
        };
        // Use a witness file because an exit status is only eight bits wide.
        let path = format!("/tmp/uitest-orphan-{}", process::getpid().unwrap().raw());
        let mut parent = || {
            let pid = fork_orphan();
            if std::fs::write(&path, pid.to_string()).is_ok() { 0 } else { 1 }
        };
        let result = (|| -> Result<u64, String> {
            let pid = spawn(&[], Run::Call(&mut parent)).map_err(|e| format!("{e:?}"))?;
            let mut status = 0;
            process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| format!("{e:?}"))?;
            if status != 0 { return Err("orphan witness writer failed".into()); }
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let orphan = text.parse::<u64>().map_err(|e| e.to_string())?;
            if orphan <= 1 { return Err("invalid orphan witness".into()); }
            Ok(orphan)
        })();
        let _ = std::fs::remove_file(path);
        match result {
            Ok(orphan) => ScenarioOutcome::fail(FailureClass::AppHung, "hung-descendants", &format!("{},{}", direct.raw(), orphan)),
            Err(error) => ScenarioOutcome::fail(FailureClass::Infrastructure, "orphan-witness", &error),
        }
    });
    let alive = signal::kill(outside.raw() as i32, 0).is_ok();
    let _ = signal::kill(outside.raw() as i32, signal::SIGKILL);
    let mut status = 0;
    let _ = process::waitpid(outside.raw() as i32, &mut status, 0);
    check(alive, "inner sweep killed the outer helper")?;
    let ScenarioOutcome::Fail { class: FailureClass::AppHung, operation, evidence, .. } = result else {
        return breenix_suite::suite::fail(format!("missing hung descendants: {result:?}"));
    };
    check(operation == "hung-descendants", "wrong teardown outcome")?;
    let pids = evidence.split(',').map(str::parse::<u64>).collect::<Result<Vec<_>,_>>().map_err(|_| "bad descendant witness")?;
    check(pids.len() == 2, "missing descendant witness")?;
    for pid in pids {
        check(pid > 1 && !std::path::Path::new(&format!("/proc/{pid}/stat")).exists(), "inner descendant survived teardown")?;
    }
    Ok(())
}

pub fn inner_records_contained() -> CaseResult {
    let result = run(&["label"], &mut |records| {
        let frame = format!("SUITE inner FRAME scenario/one label def=0123abcd ref=formula source=window:main commit=1 epoch=2 w=3 h=4 sha256={} result=match", "a".repeat(64));
        let tree = format!("SUITE inner TREE scenario/one label window=main commit=1 nodes=2 sha256={}", "b".repeat(64));
        let input = "SUITE inner INPUT scenario/one sent=1 processed=1 handled=0 rejected=0 lost=0";
        for line in [frame.as_str(), tree.as_str(), input] {
            if let Err(error) = records.record(line) {
                return ScenarioOutcome::fail(FailureClass::Infrastructure, "record", &error);
            }
        }
        // The inherited outer result authority has been closed by suite::spawn.
        if breenix_suite::suite::result_record(&frame).is_ok() {
            return ScenarioOutcome::fail(FailureClass::Infrastructure, "containment", "outer result writer still active");
        }
        ScenarioOutcome::pass()
    });
    check(classify(&Expected::Pass, &result).accepted(), &format!("contained scenario: {result:?}"))?;
    let records = result.records();
    check(records.len() == 3, "inner outcome lost records")?;
    for kind in ["FRAME", "TREE", "INPUT"] {
        check(records.iter().filter(|r| r.starts_with(&format!("SUITE inner {kind} scenario/one "))).count() == 1,
              "inner outcome record differs")?;
    }
    // A leaked record also fails this outer case: its suite/case bindings differ and it declares no checks.
    Ok(())
}
