extern crate self as libuitest;
#[path = "../libs/libuitest/src/schema.rs"] mod frame_schema;
mod frame_case {
    pub type CaseResult = Result<(), String>;
    pub fn fail<T>(message: impl Into<String>) -> Result<T, String> { Err(message.into()) }
}
#[path = "ui_frames/schema_cases.rs"] mod cases;
#[test] fn mask() { cases::mask().unwrap(); }
#[test] fn mask_cap() { cases::mask_cap().unwrap(); }
#[test] fn unknown_field() { cases::unknown_field().unwrap(); }
#[test] fn formula_params() { cases::formula_params().unwrap(); }
#[test] fn schema_version() { cases::schema_version().unwrap(); }
#[test] fn structural_vacuous() { cases::structural_vacuous().unwrap(); }
