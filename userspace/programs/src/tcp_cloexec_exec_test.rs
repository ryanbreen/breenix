//! TCP FD_CLOEXEC-across-exec() survival test (std version)
//!
//! Two-sided regression test for #707: `FdTable::close_cloexec()` (the path
//! `exec()` uses to retire `FD_CLOEXEC`-marked fds) had no arm for
//! `FdKind::TcpListener`, so a listening socket fd marked `FD_CLOEXEC` that
//! survived past an `exec()` was dropped from the fd table without ever
//! calling `tcp_listener_ref_dec()` -- the exact same class of asymmetry
//! #724 (M1) found on the `dup()` inc side, but on the `exec()` dec side.
//! `close_cloexec()`'s `FdKind::TcpListener` arm was added by PR #726
//! (commit `9db2cae0`); this test is the proof the issue's own "suggested
//! fix" section asked for and PR #726 did not include.
//!
//! Mechanism under test:
//!   1. Bind + listen on a TCP port (ref_count == 1).
//!   2. Mark the listener fd `FD_CLOEXEC` via `fcntl(F_SETFD)`.
//!   3. `fork()`. `FdTable::clone()`'s `FdKind::TcpListener` arm (the fork
//!      path, `kernel/src/ipc/fd.rs`, read directly for this test) already
//!      calls `tcp_listener_ref_inc()` on the cloned entry -- ref_count ==
//!      2 immediately after fork, one owner per process.
//!   4. Child `exec()`s `simple_exit0` by bare name. Bare-name resolution
//!      first tries `/bin/<name>` on ext2, then falls back to the raw test
//!      disk's `get_test_binary(<name>)`; `simple_exit0` is packed into that
//!      disk unconditionally, so the test does not depend on BusyBox/ext2
//!      coreutils such as `/sbin/true` being present. It exits 0 after doing
//!      nothing, matching the issue's own "suggested fix" wording ("exec a
//!      child that does nothing but exit"; claim-lint:ok: direct quote of
//!      issue #707's own suggested-fix text). The FD_CLOEXEC flag survived
//!      the fork (it's part of the cloned fd table entry), so the child's
//!      own copy of the listener fd must be retired by `close_cloexec()`
//!      during this exec.
//!   5. Parent `waitpid()`s the child and checks its exit code is exactly
//!      0, distinguishing a real `simple_exit0` run from a silently failed
//!      `exec()` -- the latter exits 1 and would otherwise look identical
//!      to "the fd leaked" on the ref-count check below (see 707-mutation.md
//!      for the single-arm revert that is the actual red/green evidence).
//!   6. Parent closes its OWN copy of the listener fd. If `close_cloexec()`
//!      correctly decremented at step 4, ref_count is 1 at this point and
//!      this close takes it to 0, genuinely retiring the listener.
//!   7. A fresh `bind()` to the same port must now succeed -- the real,
//!      external, port-level observable (mirrors tcp_dup_listener_test's
//!      own Step 5 rebind check for #724). Under the pre-#726 bug, step 4
//!      leaks a reference, so this close only takes ref_count from 2 to 1
//!      and the port is still held; the rebind fails.
//!
//! Marker sequence:
//!   TCP_CLOEXEC_EXEC_TEST_PASSED  -- the fd_table cloexec arm decremented
//!                                    the listener's ref_count exactly once
//!                                    across the child's exec(); the port is
//!                                    genuinely free after the parent's own
//!                                    close.
//!   TCP_CLOEXEC_EXEC_TEST_FAILED  -- any step below did not hold.

use std::process;

use libbreenix::io;
use libbreenix::io::fd_flags::FD_CLOEXEC;
use libbreenix::process::{execv, fork, waitpid, wexitstatus, wifexited, ForkResult};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};

const PORT: u16 = 9112;
/// `simple_exit0` exits successfully after doing nothing and is available
/// through the bare-name raw-test-disk fallback even when BusyBox/ext2
/// coreutils are absent -- the child the issue's own suggested-fix wording
/// asks for ("exec a child that does nothing but exit") -- claim-lint:ok:
/// direct quote of issue #707's own suggested-fix text.
const EXEC_TARGET_EXIT_CODE: i32 = 0;

fn fail(msg: &str) -> ! {
    println!("  FAIL: {}", msg);
    println!("TCP_CLOEXEC_EXEC_TEST_FAILED");
    process::exit(1);
}

fn main() {
    println!("=== TCP FD_CLOEXEC exec() Survival Test (#707) ===");

    // Step 1: create, bind, listen.
    println!("\nStep 1: bind + listen on port {}...", PORT);
    let listener_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  socket() returned error: {:?}", e);
            fail("socket() failed");
        }
    };
    let local_addr = SockAddrIn::new([0, 0, 0, 0], PORT);
    if let Err(e) = socket::bind_inet(listener_fd, &local_addr) {
        println!("  bind() returned error: {:?}", e);
        fail("bind() failed");
    }
    if let Err(e) = socket::listen(listener_fd, 128) {
        println!("  listen() returned error: {:?}", e);
        fail("listen() failed");
    }
    println!("  PASS: bound and listening (fd={})", listener_fd.raw() as i32);

    // Step 2: mark FD_CLOEXEC on the listener fd.
    println!("\nStep 2: mark the listener fd FD_CLOEXEC...");
    if let Err(e) = io::fcntl_setfd(listener_fd, FD_CLOEXEC) {
        println!("  fcntl(F_SETFD) returned error: {:?}", e);
        fail("fcntl(F_SETFD, FD_CLOEXEC) failed");
    }
    match io::fcntl_getfd(listener_fd) {
        Ok(flags) if flags == FD_CLOEXEC as i64 => {
            println!("  PASS: FD_CLOEXEC is set on the listener fd");
        }
        Ok(flags) => {
            println!("  fcntl(F_GETFD) returned flags={}", flags);
            fail("FD_CLOEXEC was not actually set on the listener fd");
        }
        Err(e) => {
            println!("  fcntl(F_GETFD) returned error: {:?}", e);
            fail("fcntl(F_GETFD) failed");
        }
    }

    // Step 3: fork. FdTable::clone() increments TcpListener ref_count for
    // the child's copy (verified directly against kernel/src/ipc/fd.rs) --
    // this is NOT the gap #707 is about; the gap is exec()'s retirement
    // side, exercised next.
    println!("\nStep 3: fork()...");
    let fork_result = match fork() {
        Ok(result) => result,
        Err(_) => {
            fail("fork() failed");
        }
    };

    match fork_result {
        ForkResult::Child => {
            // Child: exec() simple_exit0 by bare name, allowing the raw
            // test-disk fallback to find it even when BusyBox/ext2
            // coreutils are absent. It exits successfully without other
            // I/O. The cloned listener fd is still FD_CLOEXEC (the flag is
            // part of the cloned fd table entry), so this exec must retire
            // the child's copy through close_cloexec() -- the exact path
            // #707 is about.
            let program = b"simple_exit0\0";
            let arg0 = b"simple_exit0\0";
            let argv: [*const u8; 2] = [arg0.as_ptr(), std::ptr::null()];
            let _ = execv(program, argv.as_ptr());

            // Only reached if exec() itself failed to load the program.
            // The parent's waitpid check below (exit code != 0) also
            // catches this, but exit distinctly here for a clearer log.
            println!("  CHILD: exec(simple_exit0) failed");
            process::exit(1);
        }
        ForkResult::Parent(child_pid) => {
            // Step 4/5: wait for the child, confirm it actually reached
            // simple_exit0 (exit code 0) rather than exec() silently
            // failing, which would otherwise be indistinguishable from
            // "the fd leaked" on the ref-count check below.
            println!(
                "\nStep 4: parent waiting for child (pid={})...",
                child_pid.raw()
            );
            let mut status: i32 = 0;
            if let Err(e) = waitpid(child_pid.raw() as i32, &mut status, 0) {
                println!("  waitpid() returned error: {:?}", e);
                fail("waitpid() on the exec'd child failed");
            }
            if !wifexited(status) || wexitstatus(status) != EXEC_TARGET_EXIT_CODE {
                println!(
                    "  child did not exit with code {} (wifexited={}, status={:#x})",
                    EXEC_TARGET_EXIT_CODE,
                    wifexited(status),
                    status
                );
                fail(
                    "the child never reached simple_exit0 -- exec() itself failed, so this is \
                     not a valid exercise of close_cloexec()",
                );
            }
            println!(
                "  PASS: child exec'd simple_exit0 and exited with code {}",
                EXEC_TARGET_EXIT_CODE
            );

            // Step 6: close the PARENT's own copy. If close_cloexec()
            // correctly decremented the child's copy at exec() time (step
            // 3/4), ref_count is 1 here and this close takes it to 0,
            // genuinely retiring the listener. Under the pre-#726 bug,
            // ref_count would still be 2 (the child's exec leaked its
            // reference), and this close only takes it to 1 -- the port
            // stays held.
            println!("\nStep 5: close the parent's own listener fd...");
            if let Err(e) = io::close(listener_fd) {
                println!("  close() returned error: {:?}", e);
                fail("close() of the parent's own listener fd failed");
            }
            println!("  Parent's listener fd closed");

            // Step 7: the real external observable -- port-level, not
            // fd-level. Mirrors tcp_dup_listener_test's own rebind check
            // for #724.
            println!(
                "\nStep 6: rebind port {} -- must succeed if the listener was genuinely retired...",
                PORT
            );
            let rebind_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
                Ok(fd) => fd,
                Err(e) => {
                    println!("  socket() for rebind check returned error: {:?}", e);
                    fail("socket() for rebind check failed");
                }
            };
            match socket::bind_inet(rebind_fd, &local_addr) {
                Ok(()) => {
                    println!(
                        "  PASS: port {} was free after the parent's close -- the child's \
                         cloexec'd copy was genuinely released across exec()",
                        PORT
                    );
                    let _ = io::close(rebind_fd);
                }
                Err(e) => {
                    println!("  bind() after close returned error: {:?}", e);
                    let _ = io::close(rebind_fd);
                    fail(
                        "port was still held after the parent's own close -- the child's \
                         cloexec'd listener fd leaked a reference across exec() (#707 \
                         regression: close_cloexec() did not call tcp_listener_ref_dec())",
                    );
                }
            }

            println!("\n=== All TCP FD_CLOEXEC exec() tests passed! ===");
            println!("TCP_CLOEXEC_EXEC_TEST_PASSED");
            process::exit(0);
        }
    }
}
