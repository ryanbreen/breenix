//! Inner-executor checks; the outer table declares no inner record obligations.
use breenix_suite::suite::{check, CaseResult};
use libbreenix::process;
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
