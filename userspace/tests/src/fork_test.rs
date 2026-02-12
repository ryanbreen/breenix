//! Fork test program (std version)
//!
//! Tests basic fork() and exec() syscalls.
//! - Parent forks, child execs hello_time.elf, parent iterates and exits.

use libbreenix::process::{fork, execv, getpid, ForkResult};

fn main() {
    let pid_before = getpid().unwrap().raw() as i32;
    println!("Before fork - PID: {}", pid_before);

    println!("Calling fork()...");
    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            let pid_after = getpid().unwrap().raw() as i32;
            println!("Fork returned value: 0");
            println!("DETECTED: fork_result == 0, this is the CHILD process");
            println!("CHILD: Fork returned 0");
            println!("CHILD: PID after fork: {}", pid_after);

            // Exec hello_time.elf in the child process
            println!("CHILD: Executing hello_time.elf...");
            let path = b"/bin/hello_time\0";
            let argv: [*const u8; 1] = [std::ptr::null()];
            let _ = execv(path, argv.as_ptr());

            // If exec succeeds, this code should never run
            println!("CHILD: ERROR - exec returned, this shouldn't happen!");

            println!("CHILD: Exiting with code 42");
            std::process::exit(42);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            let fork_result = child_pid.raw() as i32;
            let pid_after = getpid().unwrap().raw() as i32;
            println!("Fork returned value: {}", fork_result);
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
        }
        Err(_) => {
            println!("ERROR: fork() failed");
            std::process::exit(1);
        }
    }
}
