use breenix_test_runner::run_test;

#[test]
fn bss_isolation() {
    let run = run_test("bss_isolation").unwrap();
    // Test that two processes can independently modify their .bss sections
    // Each process writes a different value and we verify both outputs appear
    run.assert_marker_sequence(&["P1=42", "P2=99"]);
}