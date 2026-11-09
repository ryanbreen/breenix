#[path = "../libs/libuitest/src/arena.rs"] pub mod arena;
#[path = "../libs/libuitest/src/artifacts.rs"] pub mod artifacts;
#[path = "../libs/libuitest/src/messages.rs"] pub mod messages;
extern crate self as libuitest;
extern crate self as breenix_suite;
#[path = "../libs/libuitest/src/outcome.rs"] pub mod outcome;
pub use outcome::{FailureClass, ScenarioOutcome};
#[path = "../libs/libuitest/src/schema.rs"] pub mod schema;
#[path = "../libs/libuitest/src/compare.rs"] pub mod compare;
#[path = "../libs/libuitest/src/formula.rs"] pub mod formula;
#[path = "../libs/libuitest/src/frames.rs"] pub mod frames;
#[path = "../libs/libuitest/src/client.rs"] mod client;
pub use client::RecordClient;
#[path = "ui_frames/formula_pattern.rs"] mod formula_pattern;
#[path = "ui_frames/reference_cases.rs"] pub mod reference_cases;
#[path = "../libs/libbreenix/src/suite_result.rs"] pub mod result;
pub mod suite {
    pub use crate::result;
    pub fn result_record(_: &str) -> Result<(), String> { Ok(()) }
}
#[path = "../libs/libuitest/src/schema.rs"] pub mod frame_schema;
mod frame_case {
    pub type CaseResult = Result<(), String>;
    pub fn fail<T>(message: impl Into<String>) -> Result<T, String> { Err(message.into()) }
}
#[path = "ui_frames/schema_cases.rs"] mod cases;
#[test] fn mask() {
    cases::mask().unwrap();
    let rows = cases::fixtures();
    let row = rows.iter().find(|r| r["name"] == "valid-mask").unwrap();
    assert_eq!(cases::verdict(row).unwrap().definition, row["check"]);
}
#[test] fn mask_cap() { cases::mask_cap().unwrap(); }
#[test] fn unknown_field() { cases::unknown_field().unwrap(); }
#[test] fn formula_params() { cases::formula_params().unwrap(); }
#[test] fn schema_version() { cases::schema_version().unwrap(); }
#[test] fn structural_vacuous() { cases::structural_vacuous().unwrap(); }

#[test] fn schema_binding() {
    reference_cases::schema_binding().unwrap();
    let client = RecordClient::contained("ui-testing", "frames/schema-binding", &[]).unwrap();
    assert!(matches!(client.finish(ScenarioOutcome::pass()), ScenarioOutcome::Pass { .. }));
}
