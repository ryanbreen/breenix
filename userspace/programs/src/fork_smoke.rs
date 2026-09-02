//! Fork smoke launcher -- the boot-path fork() caller (#745).
//!
//! claim-lint:ok: the pre-#745 refusal's observed effect
//! (`[FORK_SMOKE:FORK_FAILED ENOMEM]`) is quoted verbatim in
//! docs/planning/745-x86-fork/serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt
//! x86 userland otherwise has no live production caller of `fork()` proven
//! to run on a per-boot basis (`bsh`'s own three fork() call sites had never
//! executed on x86 in production before this program existed -- #745
//! precheck C13; the refusal's observed effect is quoted in docs/planning/745-x86-fork/serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt).
//! This process forces a real fork()+CoW+voluntary-yield+
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
//! forcing a real CoW write fault on EACH side independently while the frame
//! is still genuinely shared. The isolation receipt itself is then taken on a
//! SECOND probe, `CHILD_ONLY_PROBE`, which only the child ever writes: after
//! reaping the child the parent reads it and requires the PRE-FORK zero.
//! claim-lint:ok: both sides' faults and the receipt's own failure mode were
//! observed, not assumed -- see the mutation run cited two paragraphs below,
//! docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt
//!
//! The two-probe split is what makes the receipt order-independent (#745
//! review round 2, M2). Reading back the parent's own sentinel from a probe
//! BOTH sides wrote cannot fail on interleaving alone: the child is queued
//! with `spawn_front`, so on a broken kernel that left one writable frame
//! shared, the child's write plausibly lands FIRST and the parent's later
//! write overwrites it -- the parent then reads its own value back and the
//! receipt reports OK on a genuinely broken kernel. `CHILD_ONLY_PROBE` has no
//! such hole in either direction: the parent never writes it, the child's
//! write is sequenced before its exit and therefore before the reap, so a
//! shared frame makes the child's value visible to the parent no matter how
//! the two processes interleave. The parent's own-sentinel read is kept as
//! well, as the check that the parent's copy is private *and* writable.
//! claim-lint:ok: the "reports OK on a genuinely broken kernel" half is a run,
//! not a hypothetical --
//! docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt
//!
//! claim-lint:ok: the two-probe receipt was run against a deliberately broken
//! kernel (child mapped with the parent's original writable flags) before it
//! was believed. It printed
//! `[FORK_SMOKE:COW_ISOLATION_CORRUPTED probe=0xfeedfeed child_only=0xc0ffeeee]`
//! -- probe=0xfeedfeed is the parent reading its OWN sentinel back, i.e. the
//! single-probe receipt this replaced would have reported OK on that kernel.
//! docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt
//!
//! The child takes the mirror-image reading first, before it writes anything:
//! a kernel that left the PARENT's page writable (so the parent's post-fork
//! write never faulted and landed in the frame the child still shares) is
//! invisible to every parent-side check, because the parent would read back
//! exactly the value it wrote. On a correct kernel the child's pre-write read
//! is the pre-fork 0 regardless of scheduling, since the parent's write can
//! only reach the parent's private copy -- that is the no-false-positive
//! direction, and it holds unconditionally. Detection is NOT
//! order-independent, unlike `CHILD_ONLY_PROBE` above: this arm only fires
//! if the parent's post-fork write to `SHARED_WRITE_PROBE` lands before the
//! child's pre-write read, and the child is queued with `spawn_front`
//! (front of the ready queue), so a child-first schedule is the likely one
//! and this arm then does not fire at all. It is best-effort: the
//! asymmetric-corruption shape it targets is caught probabilistically, not
//! deterministically. The round's own mutation run shows exactly this --
//! `side=child` never printed; only the `CHILD_ONLY_PROBE`-backed
//! `side`-less line did (see the artifact below).
//! claim-lint:ok: same mutation run as above --
//! docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt
//!
//! This is what makes the isolation proof functional rather than "some CoW
//! fault log line appeared": the x86 CoW *fault* path
//! (`handle_cow_fault`/`handle_cow_with_manager`/`frame_is_shared`) had
//! never executed in a zero-feature x86 build before this program existed
//! (#745 precheck C3), and a broken refcount/isolation check would silently
//! corrupt parent/child memory rather than crash -- a child that only
//! yields and exits proves nothing about that. The gate pins the fault's
//! OCCURRENCE separately, on the kernel's own `[COW FAULT #0] addr=` line
//! (C3(2)); this program answers the other half, what the fault handler DID.
//! claim-lint:ok: "had never executed in a zero-feature x86 build" is precheck
//! C3's own census, docs/planning/745-x86-fork/precheck.md; the receipt's
//! ability to fail is the mutation run cited above.

use libbreenix::process::{fork, getpid, waitpid, wexitstatus, wifexited, yield_now, ForkResult};

/// The exit code the child uses on its clean-exit path, distinguishing a
/// genuine reap from a fabricated one in the gate's own assertions.
const CHILD_EXIT_CODE: i32 = 37;

/// Sentinel value each side writes to `SHARED_WRITE_PROBE`.
const PARENT_PROBE_VALUE: u64 = 0xFEED_FEED;
const CHILD_PROBE_VALUE: u64 = 0xC0FF_EEEE;

/// Writable global the parent and child both mutate post-fork, forcing a
/// real CoW fault on each side while the frame is still shared (see module
/// doc).
static mut SHARED_WRITE_PROBE: u64 = 0;

/// Writable global ONLY the child ever writes. The parent reads it after the
/// reap and requires the pre-fork zero, which is the order-independent half
/// of the isolation receipt (see module doc).
/// claim-lint:ok: this file is the only writer of this symbol, and the receipt
/// was reddened by mutation --
/// docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt
static mut CHILD_ONLY_PROBE: u64 = 0;

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

            // The other direction of the same leak (#745 review round 2, M2):
            // read the probe BEFORE writing it. On a correct kernel this is
            // always the pre-fork 0 -- the parent's own post-fork write faults
            // into the parent's PRIVATE copy and can never be visible here.
            // Observing PARENT_PROBE_VALUE means the parent's page stayed
            // writable-shared, which the parent-side check below cannot see
            // (the parent would still read its own value back).
            // claim-lint:ok: that exact parent-side blindness was observed on a
            // mutated kernel (probe=0xfeedfeed, the parent's own sentinel) --
            // docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt
            let pre = unsafe { SHARED_WRITE_PROBE };
            if pre == PARENT_PROBE_VALUE {
                println!(
                    "[FORK_SMOKE:COW_ISOLATION_CORRUPTED probe={:#x} side=child]",
                    pre
                );
            }

            // Force a CoW write fault on the child's own private copy of
            // this page. CHILD_ONLY_PROBE carries the same value: if the
            // kernel left one writable frame shared, this write is what the
            // parent sees after the reap.
            unsafe {
                SHARED_WRITE_PROBE = CHILD_PROBE_VALUE;
                CHILD_ONLY_PROBE = CHILD_PROBE_VALUE;
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

                    // The child has now run to exit and been reaped, so both
                    // reads are race-free and, on CHILD_ONLY_PROBE,
                    // order-independent (see module doc): the child's write
                    // is sequenced before its exit, so a shared frame would
                    // make CHILD_PROBE_VALUE visible here no matter how the
                    // two processes interleaved. `shared` additionally
                    // requires the parent's own copy to be private AND
                    // writable.
                    let shared = unsafe { SHARED_WRITE_PROBE };
                    let child_only = unsafe { CHILD_ONLY_PROBE };
                    if shared == PARENT_PROBE_VALUE && child_only == 0 {
                        println!(
                            "[FORK_SMOKE:COW_ISOLATION_OK probe={:#x} child_only={:#x}]",
                            shared, child_only
                        );
                    } else {
                        println!(
                            "[FORK_SMOKE:COW_ISOLATION_CORRUPTED probe={:#x} child_only={:#x}]",
                            shared, child_only
                        );
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
