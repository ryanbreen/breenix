//! Fork test program (std version)
//!
//! Tests basic fork() and exec() syscalls.
//! - Parent forks, child execs hello_time.elf, parent iterates and exits.

extern "C" {
    fn fork() -> i32;
    fn getpid() -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
}

fn main() {
    let pid_before = unsafe { getpid() };
    println!("Before fork - PID: {}", pid_before);

    println!("Calling fork()...");
    let fork_result = unsafe { fork() };

    println!("Fork returned value: {}", fork_result);

    let pid_after = unsafe { getpid() };

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        println!("DETECTED: fork_result == 0, this is the CHILD process");
        println!("CHILD: Fork returned 0");
        println!("CHILD: PID after fork: {}", pid_after);

        // Exec hello_time.elf in the child process
        println!("CHILD: Executing hello_time.elf...");
        let path = b"/userspace/tests/hello_time.elf\0";
        let argv: [*const u8; 1] = [std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];
        let exec_result = unsafe {
            execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr())
        };

        // If exec succeeds, this code should never run
        println!("CHILD: ERROR - exec returned, this shouldn't happen!");
        println!("CHILD: exec returned: {}", exec_result);

        println!("CHILD: Exiting with code 42");
        std::process::exit(42);
    } else if fork_result > 0 {
        // ========== PARENT PROCESS ==========
        println!("DETECTED: fork_result != 0, this is the PARENT process");
        println!("PARENT: Fork returned child PID: {}", fork_result);
        println!("PARENT: PID after fork: {}", pid_after);

        // Do some parent work
        for i in 0..3u64 {
            println!("PARENT: iteration {}", i);
            // Small delay
            for _ in 0..1000000u64 {
                unsafe { std::ptr::read_volatile(&0u8); }
            }
        }

        println!("PARENT: Exiting with code 0");
        std::process::exit(0);
    } else {
        println!("ERROR: fork() failed with error: {}", fork_result);
        std::process::exit(1);
    }
}
