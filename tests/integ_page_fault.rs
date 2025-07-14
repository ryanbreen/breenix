use breenix_test_runner::{run_test, markers};

#[test]
fn page_fault() {
    let run = run_test("page_fault").unwrap();
    run.assert_marker(markers::PF_OK);
}