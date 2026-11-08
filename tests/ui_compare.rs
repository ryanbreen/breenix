extern crate self as libuitest;
#[path = "../libs/libuitest/src/compare.rs"]
pub mod compare;
#[path = "../libs/libuitest/src/outcome.rs"]
pub mod outcome;
#[path = "../userspace/programs/src/uitest_suite/comparison_cases.rs"]
mod cases;
#[test] fn exact() { cases::exact_match().unwrap(); }
#[test] fn pixel() { cases::one_pixel_fails().unwrap(); }
#[test] fn tolerance() { cases::channel_tolerance().unwrap(); }
#[test] fn cap() { cases::max_differing().unwrap(); }
#[test] fn regions() { cases::regions_same_frame().unwrap(); }
#[test] fn sizes() { cases::size_mismatch().unwrap(); }

#[test] fn timing_case_is_linked() { let _ = cases::compare_cost as fn() -> Result<(), String>; }
