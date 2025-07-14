use breenix_test_runner::run_test;

#[test]
fn multiple_processes() {
    let run = run_test("multiple_processes").unwrap();
    // The test should create 5 processes that each print "Hello from userspace!"
    run.assert_count("Hello from userspace!", 5);
}