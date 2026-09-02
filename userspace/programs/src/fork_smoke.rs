//! Fork smoke launcher -- the boot-path fork() caller (#745).
//!
//! x86 userland otherwise has no live production caller of `fork()` proven
//! to run on a per-boot basis (`bsh`'s own three fork() call sites had never
//! executed on x86 in production before this program existed -- #745
//! precheck C13). This process forces a real fork()+CoW+voluntary-yield+
//! exit+reap round trip so #745's fix (the interrupt-masking restructure of
//! `sys_fork_with_parent_context` and the de-gated CoW block, see
//! `docs/planning/745-x86-fork/`) is proven by a live boot, not just a
//! build. Arch-neutral, no `target_arch` cfg -- mirrors `exec_smoke.rs`'s
//! own precedent (#721).
//!
//! The child forces at least one voluntary yield BEFORE exiting --
//! deliberately, so the freshly-published, never-preempted child thread
//! goes through a real context-switch/reschedule round trip. That is
//! exactly the scenario a masked-interrupt regression in the fork syscall
//! path would need to survive (#745 precheck C1/C4/C9): the entire fork
//! critical section running interrupt-disabled would still boot and pass a
//! single uncontended smoke run, but hang or deadlock the moment some other
//! thread is preempted mid-lock-hold during a concurrent fork call.
//!
//! Both parent and child write to `SHARED_WRITE_PROBE` after fork() returns,
//! forcing a real CoW write fault on EACH side independently, then the
//! parent -- only after successfully reaping the child, so there is no
//! read/write race -- reads the probe back and requires it to still hold
//! ITS OWN value. This is a functional isolation proof, not just "some CoW
//! fault log line appeared": the x86 CoW *fault* path
//! (`handle_cow_fault`/`handle_cow_with_manager`/`frame_is_shared`) had
//! never executed in a zero-feature x86 build before this program existed
//! (#745 precheck C3), and a broken refcount/isolation check would silently
//! corrupt parent/child memory rather than crash -- a child that only
//! yields and exits proves nothing about that.

use libbreenix::process::{fork, getpid, waitpid, wexitstatus, wifexited, yield_now, ForkResult};

/// The exit code the child uses on its clean-exit path, distinguishing a
/// genuine reap from a fabricated one in the gate's own assertions.
const CHILD_EXIT_CODE: i32 = 37;

/// Sentinel value each side writes to `SHARED_WRITE_PROBE`.
const PARENT_PROBE_VALUE: u64 = 0xFEED_FEED;
const CHILD_PROBE_VALUE: u64 = 0xC0FF_EEEE;

/// Writable global the parent and child both mutate post-fork, forcing a
/// real CoW fault on each side (see module doc).
static mut SHARED_WRITE_PROBE: u64 = 0;

/// Set once the child branch has begun its own work. If fork() were to
/// (incorrectly) resume a second execution context into this same branch --
/// the historical "fork() returns twice into the same logical branch" bug
/// shape -- the second entrant observes the flag already set instead of
/// silently repeating (and possibly corrupting) the first child's work.
static mut CHILD_ENTERED: bool = false;

fn main() {
    println!("[FORK_SMOKE:LAUNCH]");

    match fork() {
        Ok(ForkResult::Child) => {
            if unsafe { CHILD_ENTERED } {
                println!("[FORK_SMOKE:CHILD_UNEXPECTED_RETURN]");
                loop {
                    core::hint::spin_loop();
                }
            }
            unsafe {
                CHILD_ENTERED = true;
            }

            // Force a CoW write fault on the child's own private copy of
            // this page.
            unsafe {
                SHARED_WRITE_PROBE = CHILD_PROBE_VALUE;
            }

            let pid = getpid().map(|p| p.raw()).unwrap_or(0);
            println!("[FORK_SMOKE:CHILD pid={}]", pid);

            // At least one voluntary yield before exiting -- forces the
            // freshly-published child thread through a real reschedule
            // round trip (see module doc).
            let _ = yield_now();

            std::process::exit(CHILD_EXIT_CODE);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Force a CoW write fault on the parent's own private copy of
            // the same page the child just wrote to.
            unsafe {
                SHARED_WRITE_PROBE = PARENT_PROBE_VALUE;
            }

            let mut status: i32 = 0;
            match waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0) {
                Ok(_) => {
                    let code = if wifexited(status) {
                        wexitstatus(status)
                    } else {
                        -1
                    };

                    // The child has now fully run and been reaped, so this
                    // read is race-free: if CoW isolation is correct, the
                    // child's earlier write can only have affected the
                    // child's own private copy, and the parent must still
                    // observe its OWN value here.
                    let observed = unsafe { SHARED_WRITE_PROBE };
                    if observed == PARENT_PROBE_VALUE {
                        println!("[FORK_SMOKE:COW_ISOLATION_OK probe={:#x}]", observed);
                    } else {
                        println!("[FORK_SMOKE:COW_ISOLATION_CORRUPTED probe={:#x}]", observed);
                    }

                    println!(
                        "[FORK_SMOKE:PARENT_REAPED child={} code={}]",
                        child_pid.raw(),
                        code
                    );
                }
                Err(e) => {
                    println!("[FORK_SMOKE:REAP_FAILED {}]", e);
                }
            }
        }
        Err(e) => {
            println!("[FORK_SMOKE:FORK_FAILED {}]", e);
        }
    }
}
