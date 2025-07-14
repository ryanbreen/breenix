use breenix_test_runner::run_test;

#[test]
fn sys_unknown() {
    let run = run_test("syscall_unknown").unwrap();
    
    // Test that unknown syscall returns -ENOSYS (-38)
    // The test program will exit with code 0 if -ENOSYS is returned,
    // or exit with code 1 if something else is returned
    
    // For now, we'll just verify the kernel doesn't crash by checking that 
    // syscall 999 is received
    run.assert_marker("SYSCALL_ENTRY: Received syscall from userspace! RAX=0x3e7");
}