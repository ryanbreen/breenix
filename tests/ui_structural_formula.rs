extern crate self as libuitest;
#[path = "../libs/libuitest/src/compare.rs"] pub mod compare;
#[path = "../libs/libuitest/src/outcome.rs"] pub mod outcome;
#[path = "../libs/libuitest/src/schema.rs"] pub mod schema;
#[path = "../libs/libuitest/src/formula.rs"] pub mod formula;
#[path = "ui_frames/structural_cases.rs"] mod cases;
#[test] fn uniform() { cases::uniform().unwrap(); }
#[test] fn outside() { cases::outside().unwrap(); }
#[test] fn count() { cases::count().unwrap(); }
#[test] fn formula_matches_stored() { cases::formula().unwrap(); }
#[path = "ui_frames/formula_pattern.rs"] mod formula_pattern;
