//! Signal test program (std version)
//!
//! Tests basic signal functionality:
//! 1. kill() syscall to send SIGTERM to child
//! 2. Default signal handler (terminate)

use libbreenix::process::{fork, getpid, waitpid, yield_now, wtermsig, ForkResult};
use libbreenix::signal::{kill, SIGTERM};

// Execute a SYSCALL ending at the last canonical user byte. Its hardware
// return RCX is non-canonical; SYSRET must be rejected and IRETQ's #GP must
// kill this child with SIGSEGV while the parent and the boot keep running.
#[cfg(target_arch = "x86_64")]
fn check_noncanonical_syscall_return() {
    use std::os::unix::fs::PermissionsExt;
    let mut elf = [0u8; 4096];
    elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    elf[18..20].copy_from_slice(&62u16.to_le_bytes());
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[24..32].copy_from_slice(&0x7fff_ffff_fff9u64.to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&2u16.to_le_bytes());
    elf[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    elf[68..72].copy_from_slice(&5u32.to_le_bytes()); // readable/executable
    elf[80..88].copy_from_slice(&0x7fff_ffff_f000u64.to_le_bytes());
    elf[96..104].copy_from_slice(&4096u64.to_le_bytes());
    elf[104..112].copy_from_slice(&4096u64.to_le_bytes());
    elf[112..120].copy_from_slice(&4096u64.to_le_bytes());
    // An empty PT_LOAD is legal and must not underflow the segment end.
    elf[120..124].copy_from_slice(&1u32.to_le_bytes());
    elf[136..144].copy_from_slice(&0x4000_0000u64.to_le_bytes());
    elf[168..176].copy_from_slice(&4096u64.to_le_bytes());
    elf[4089..].copy_from_slice(&[0xb8, 39, 0, 0, 0, 0x0f, 0x05]); // getpid; syscall
    let path = "/tmp/syscall_edge.elf";
    std::fs::write(path, elf).expect("write syscall edge executable");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod syscall edge executable");
    match fork().expect("fork syscall edge child") {
        ForkResult::Child => {
            let argv = [b"syscall_edge\0".as_ptr(), std::ptr::null()];
            let error = libbreenix::process::execv(b"/tmp/syscall_edge.elf\0", argv.as_ptr());
            panic!("exec syscall edge executable failed: {:?}", error);
        }
        ForkResult::Parent(child) => {
            let mut status = 0;
            let reaped = waitpid(child.raw() as i32, &mut status, 0).expect("wait syscall edge child");
            assert_eq!(reaped.raw(), child.raw());
            assert_eq!(wtermsig(status), libbreenix::signal::SIGSEGV);
            std::fs::remove_file(path).expect("remove syscall edge executable");
            println!("USER_NONCANONICAL_SYSCALL_PASSED");
        }
    }
}

fn main() {
    println!("=== Signal Test ===");

    #[cfg(target_arch = "x86_64")]
    match fork().expect("fork #UD child") {
        ForkResult::Child => unsafe { core::arch::asm!("ud2", options(noreturn)); },
        ForkResult::Parent(child) => {
            let mut status = 0;
            let reaped = waitpid(child.raw() as i32, &mut status, 0).expect("wait #UD child");
            assert_eq!(reaped.raw(), child.raw());
            assert_eq!(wtermsig(status), libbreenix::signal::SIGILL);
            println!("USER_UD_SIGILL_PASSED");
        }
    }

    #[cfg(target_arch = "x86_64")]
    check_noncanonical_syscall_return();

    let my_pid = getpid().unwrap().raw() as i32;
    println!("My PID: {}", my_pid);

    // Test 1: Check if process exists using kill(pid, 0)
    println!("\nTest 1: Check process exists with kill(pid, 0)");
    let ret = kill(my_pid, 0);
    if ret.is_ok() {
        println!("  PASS: Process exists");
    } else {
        println!("  FAIL: kill returned error");
    }

    // Test 2: Fork and send SIGTERM to child
    println!("\nTest 2: Fork and send SIGTERM to child");

    match fork() {
        Ok(ForkResult::Child) => {
            // Child process - loop forever, waiting for signal
            println!("  CHILD: Started, waiting for signal...");
            let child_pid = getpid().unwrap().raw() as i32;
            println!("  CHILD: My PID is {}", child_pid);

            // Busy loop - should be killed by parent
            let mut counter = 0u64;
            loop {
                counter = counter.wrapping_add(1);
                if counter % 10_000_000 == 0 {
                    println!("  CHILD: Still alive...");
                }
                // Yield to let parent run
                if counter % 100_000 == 0 {
                    let _ = yield_now();
                }
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent process
            let child_pid_i32 = child_pid.raw() as i32;
            println!("  PARENT: Forked child with PID {}", child_pid_i32);

            // Small delay to let child start
            println!("  PARENT: Waiting for child to start...");
            for i in 0..5 {
                println!("  PARENT: yield {}", i);
                let _ = yield_now();
            }
            println!("  PARENT: Done waiting, about to send signal");

            // Send SIGTERM to child
            println!("  PARENT: Sending SIGTERM to child");
            let ret = kill(child_pid_i32, SIGTERM);
            if ret.is_ok() {
                println!("  PARENT: kill() syscall succeeded");
            } else {
                println!("  PARENT: kill() failed");
                std::process::exit(1);
            }

            // Wait for child to actually terminate using waitpid
            println!("  PARENT: Waiting for child to terminate...");
            let mut status: i32 = 0;
            let result = waitpid(child_pid_i32, &mut status, 0).unwrap();

            if result.raw() as i32 == child_pid_i32 {
                // Check if child was terminated by signal
                // WTERMSIG: status & 0x7f
                let termsig = wtermsig(status);
                if termsig == SIGTERM {
                    println!("  PARENT: Child terminated by SIGTERM!");
                    println!("SIGNAL_KILL_TEST_PASSED");
                } else if termsig != 0 {
                    println!("  PARENT: Child terminated by wrong signal: {}", termsig);
                    std::process::exit(2);
                } else {
                    // Child exited normally (WIFEXITED)
                    let exit_code = (status >> 8) & 0xff;
                    println!("  PARENT: Child exited normally (not by signal), exit code: {}", exit_code);
                    std::process::exit(3);
                }
            } else {
                println!("  PARENT: waitpid returned unexpected value: {}", result.raw() as i32);
                std::process::exit(4);
            }

            println!("  PARENT: Test complete, exiting");
            std::process::exit(0);
        }
        Err(_) => {
            println!("fork failed");
            std::process::exit(1);
        }
    }
}
