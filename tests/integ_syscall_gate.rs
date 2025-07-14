use breenix_test_runner::run_test;

#[test]
fn syscall_gate() {
    let run = run_test("syscall_gate").unwrap();
    // Test that userspace can call INT 0x80 and get a response
    run.assert_marker("SYSCALL_OK");
}