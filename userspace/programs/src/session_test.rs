//! Session and process group syscall tests (std version)
//!
//! Tests POSIX session and process group syscalls:
//! - getpgid()/setpgid() - process group get/set
//! - getpgrp() - get calling process's process group
//! - getsid()/setsid() - session get/create

use libbreenix::process::{self, ForkResult};

fn fail(msg: &str) -> ! {
    println!("SESSION_TEST: FAIL - {}", msg);
    std::process::exit(1);
}

fn test_getpgid_self() {
    println!("\nTest 1: getpgid(0) returns current process's pgid");

    match process::getpgid(0) {
        Ok(pgid) => {
            let pgid_i32 = pgid.raw() as i32;
            if pgid_i32 <= 0 {
                println!("  getpgid(0) returned: {}", pgid_i32);
                fail("getpgid(0) should return positive value");
            }
            println!("  getpgid(0) = {}", pgid_i32);
            println!("  test_getpgid_self: PASS");
        }
        Err(_) => {
            fail("getpgid(0) failed");
        }
    }
}

fn test_getpgid_with_pid() {
    println!("\nTest 2: getpgid(getpid()) returns same as getpgid(0)");

    let pid = process::getpid().unwrap_or_else(|_| fail("getpid failed"));
    let pid_i32 = pid.raw() as i32;
    let pgid_0 = process::getpgid(0).unwrap_or_else(|_| fail("getpgid(0) failed"));
    let pgid_pid = process::getpgid(pid_i32).unwrap_or_else(|_| fail("getpgid(pid) failed"));

    println!("  pid = {}", pid_i32);
    println!("  getpgid(0) = {}", pgid_0.raw() as i32);
    println!("  getpgid(pid) = {}", pgid_pid.raw() as i32);

    if pgid_0.raw() != pgid_pid.raw() {
        fail("getpgid(0) should equal getpgid(getpid())");
    }

    println!("  test_getpgid_with_pid: PASS");
}

fn test_setpgid_self() {
    println!("\nTest 3: setpgid(0, 0) sets pgid to own pid");

    let pid = process::getpid().unwrap_or_else(|_| fail("getpid failed"));
    let pid_i32 = pid.raw() as i32;
    match process::setpgid(0, 0) {
        Ok(()) => {}
        Err(_) => {
            fail("setpgid(0, 0) should succeed");
        }
    }

    println!("  pid = {}", pid_i32);
    println!("  setpgid(0, 0) returned: 0");

    let pgid = process::getpgid(0).unwrap_or_else(|_| fail("getpgid failed"));
    let pgid_i32 = pgid.raw() as i32;
    println!("  getpgid(0) after setpgid = {}", pgid_i32);

    if pgid_i32 != pid_i32 {
        fail("after setpgid(0, 0), pgid should equal pid");
    }

    println!("  test_setpgid_self: PASS");
}

fn test_getpgrp() {
    println!("\nTest 4: getpgrp() returns same as getpgid(0)");

    let pgrp = process::getpgrp().unwrap_or_else(|_| fail("getpgrp failed"));
    let pgid = process::getpgid(0).unwrap_or_else(|_| fail("getpgid(0) failed"));

    println!("  getpgrp() = {}", pgrp.raw() as i32);
    println!("  getpgid(0) = {}", pgid.raw() as i32);

    if pgrp.raw() != pgid.raw() {
        fail("getpgrp() should equal getpgid(0)");
    }

    println!("  test_getpgrp: PASS");
}

fn test_getsid_self() {
    println!("\nTest 5: getsid(0) returns current session id");

    match process::getsid(0) {
        Ok(sid) => {
            let sid_i32 = sid.raw() as i32;
            if sid_i32 <= 0 {
                println!("  getsid(0) returned: {}", sid_i32);
                fail("getsid(0) should return positive value");
            }
            println!("  getsid(0) = {}", sid_i32);
            println!("  test_getsid_self: PASS");
        }
        Err(_) => {
            fail("getsid(0) failed");
        }
    }
}

fn test_getsid_with_pid() {
    println!("\nTest 6: getsid(getpid()) returns same as getsid(0)");

    let pid = process::getpid().unwrap_or_else(|_| fail("getpid failed"));
    let pid_i32 = pid.raw() as i32;
    let sid_0 = process::getsid(0).unwrap_or_else(|_| fail("getsid(0) failed"));
    let sid_pid = process::getsid(pid_i32).unwrap_or_else(|_| fail("getsid(pid) failed"));

    println!("  pid = {}", pid_i32);
    println!("  getsid(0) = {}", sid_0.raw() as i32);
    println!("  getsid(pid) = {}", sid_pid.raw() as i32);

    if sid_0.raw() != sid_pid.raw() {
        fail("getsid(0) should equal getsid(getpid())");
    }

    println!("  test_getsid_with_pid: PASS");
}

fn test_setsid_in_child() {
    println!("\nTest 7: setsid() in child creates new session");

    match process::fork() {
        Ok(ForkResult::Child) => {
            // Child process
            let my_pid = process::getpid().unwrap_or_else(|_| std::process::exit(1));
            let my_pid_i32 = my_pid.raw() as i32;
            println!("  CHILD: pid = {}", my_pid_i32);

            match process::setpgid(0, 0) {
                Ok(()) => println!("  CHILD: setpgid(0, 0) returned: 0"),
                Err(_) => println!("  CHILD: setpgid(0, 0) failed"),
            }

            match process::setsid() {
                Ok(new_sid) => {
                    println!("  CHILD: setsid() returned: {}", new_sid.raw() as i32);
                }
                Err(_) => {
                    println!("  CHILD: setsid() failed");
                    std::process::exit(1);
                }
            }

            let sid = process::getsid(0).unwrap_or_else(|_| std::process::exit(1));
            let pgid = process::getpgid(0).unwrap_or_else(|_| std::process::exit(1));

            println!("  CHILD: getsid(0) = {}", sid.raw() as i32);
            println!("  CHILD: getpgid(0) = {}", pgid.raw() as i32);

            if sid.raw() as i32 != my_pid_i32 {
                println!("  CHILD: ERROR - sid should equal pid after setsid");
                std::process::exit(1);
            }

            if pgid.raw() as i32 != my_pid_i32 {
                println!("  CHILD: ERROR - pgid should equal pid after setsid");
                std::process::exit(1);
            }

            println!("  CHILD: setsid test PASS");
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child = child_pid.raw() as i32;
            println!("  PARENT: waiting for child {}", child);

            let mut status: i32 = 0;
            match process::waitpid(child, &mut status, 0) {
                Ok(pid) => {
                    println!("  PARENT: waitpid returned: {}", pid.raw() as i32);

                    if pid.raw() as i32 != child {
                        fail("waitpid returned wrong pid");
                    }
                }
                Err(_) => {
                    fail("waitpid failed");
                }
            }

            if !process::wifexited(status) {
                println!("  PARENT: child did not exit normally, status = {}", status);
                fail("child did not exit normally");
            }

            let exit_code = process::wexitstatus(status);
            println!("  PARENT: child exit code = {}", exit_code);

            if exit_code != 0 {
                fail("child reported test failure");
            }

            println!("  test_setsid_in_child: PASS");
        }
        Err(_) => {
            fail("fork failed");
        }
    }
}

fn test_error_cases() {
    println!("\nTest 8: Error cases for invalid PIDs");

    match process::getpgid(-1) {
        Ok(pgid) => {
            println!("  getpgid(-1) = {}", pgid.raw() as i32);
            fail("getpgid(-1) should return error (negative value)");
        }
        Err(_) => {
            println!("  getpgid(-1) returned error (as expected)");
        }
    }

    match process::getsid(-1) {
        Ok(sid) => {
            println!("  getsid(-1) = {}", sid.raw() as i32);
            fail("getsid(-1) should return error (negative value)");
        }
        Err(_) => {
            println!("  getsid(-1) returned error (as expected)");
        }
    }

    println!("  test_error_cases: PASS");
}

fn main() {
    println!("=== Session Syscall Tests ===");

    test_getpgid_self();
    test_getpgid_with_pid();
    test_setpgid_self();
    test_getpgrp();
    test_getsid_self();
    test_getsid_with_pid();
    test_setsid_in_child();
    test_error_cases();

    println!("\n=== All session tests passed! ===");
    println!("SESSION_TEST_PASSED");
    std::process::exit(0);
}
