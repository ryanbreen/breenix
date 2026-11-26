//! Drive the dedicated ARM64 post-handoff fault-routing acceptance profile.

#[cfg(target_arch = "aarch64")]
fn main() {
    use libbreenix::process::{self, ForkResult};
    use libbreenix::syscall::raw;
    use std::time::{Duration, Instant};

    // Exists only with kernel feature gpu_fault_acceptance.
    const PROFILE_SYSCALL: u64 = 0x7fff_0000;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let result = unsafe { raw::syscall1(PROFILE_SYSCALL, 0) } as i64;
        if result == 0 {
            break;
        }
        assert_eq!(result, -11, "fault acceptance profile unavailable");
        assert!(
            Instant::now() < deadline,
            "GPU handoff/preparation timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    match process::fork().expect("fork faulting child") {
        ForkResult::Child => {
            println!("GPU_FAULT_TEST user syscall=0x7fff0000 handoff=1");
            // Operation 2 makes a raw EL1 write to an unmapped user address.
            // EFAULT or a normal return cannot stand in for the requested abort.
            unsafe { raw::syscall1(PROFILE_SYSCALL, 2) };
            println!("GPU_FAULT_TEST user unexpectedly returned");
            process::exit(1);
        }
        ForkResult::Parent(child) => {
            let mut status = 0;
            let reaped =
                process::waitpid(child.raw() as i32, &mut status, 0).expect("reap faulting child");
            assert_eq!(reaped.raw(), child.raw());
            assert!(
                process::wifsignaled(status),
                "child did not die by signal: {status}"
            );
            assert_eq!(
                process::wtermsig(status),
                11,
                "child did not receive SIGSEGV"
            );
            // Check a fresh syscall after wait before triggering the terminal fault.
            assert!(process::getpid().unwrap().raw() > 0);
            assert_eq!(unsafe { raw::syscall1(PROFILE_SYSCALL, 0) }, 0);
            println!("GPU_FAULT_TEST recovered signal=11 kernel=running handoff=1");
            assert_eq!(unsafe { raw::syscall1(PROFILE_SYSCALL, 1) }, 0);
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn main() {
    panic!("gpu_fault_acceptance requires ARM64");
}
