use breenix_test_runner::{run_test, markers};

#[test]
fn invalid_opcode() {
    let run = run_test("invalid_opcode").unwrap();
    run.assert_marker(markers::UD_OK);
}