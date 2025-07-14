use breenix_test_runner::{run_test, markers};

#[test]
fn divide_by_zero() {
    let run = run_test("divide_by_zero").unwrap();
    run.assert_marker(markers::DIV0_OK);
}