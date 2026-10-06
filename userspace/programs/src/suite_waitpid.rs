//! Repeated child-exit handoffs. The suite runner kills a stranded waiter and
//! reports a failure instead of leaving a testing boot waiting forever.
use libbreenix::memory::{self, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{self, Sigaction, StackT, SA_ONSTACK, SA_RESTART, SIGCHLD};
use libbreenix::suite::{case, category, check, suite, CaseResult, Suite};
use std::sync::atomic::{AtomicUsize, Ordering};

static SIGNALS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn child_exited(_: i32) {
    SIGNALS.fetch_add(1, Ordering::Relaxed);
}

fn cow_signal_stack() -> CaseResult {
    let stack = memory::mmap(
        core::ptr::null_mut(),
        16384,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )?;
    // Populate before fork. Neither parent nor child touches these pages after
    // fork: the kernel's first signal-frame write must resolve their CoW.
    // Put the stack top 128 bytes into a page, so the frame spans two pages.
    unsafe { core::ptr::write_bytes(stack, 0xa5, 16384) };
    let alt = StackT {
        ss_sp: stack as u64,
        ss_flags: 0,
        _pad: 0,
        ss_size: 8192 + 128,
    };
    signal::sigaltstack(Some(&alt), None)?;
    let mut action = Sigaction::new(child_exited);
    action.flags |= SA_RESTART | SA_ONSTACK;
    signal::sigaction(SIGCHLD, Some(&action), None)?;
    match process::fork()? {
        ForkResult::Child => process::exit(42),
        ForkResult::Parent(child) => {
            let mut status = 0;
            let waited = process::waitpid(child.raw() as i32, &mut status, 0)?;
            check(waited == child, "CoW signal stack: wrong child")?;
            check(
                process::wifexited(status) && process::wexitstatus(status) == 42,
                "CoW signal stack: wrong exit status",
            )?;
            check(
                SIGNALS.load(Ordering::Relaxed) == 1,
                "CoW signal stack: SIGCHLD handler did not run",
            )?;
        }
    }
    signal::sigaltstack(Some(&StackT::default()), None)?;
    memory::munmap(stack, 16384)?;
    Ok(())
}

fn repeat_wait(any_child: bool, caught: bool) -> CaseResult {
    if caught {
        let mut action = Sigaction::new(child_exited);
        action.flags |= SA_RESTART;
        signal::sigaction(SIGCHLD, Some(&action), None)?;
    }
    for iteration in 0..256 {
        match process::fork()? {
            ForkResult::Child => process::exit(42),
            ForkResult::Parent(child) => {
                let mut status = 0;
                let waited = process::waitpid(
                    if any_child { -1 } else { child.raw() as i32 },
                    &mut status,
                    0,
                )?;
                check(
                    waited == child,
                    &format!("iteration {iteration}: wrong child"),
                )?;
                check(
                    process::wifexited(status) && process::wexitstatus(status) == 42,
                    &format!("iteration {iteration}: wrong exit status"),
                )?;
                if caught {
                    check(
                        SIGNALS.load(Ordering::Relaxed) == iteration + 1,
                        &format!("iteration {iteration}: missing SIGCHLD"),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn specific() -> CaseResult {
    repeat_wait(false, false)
}
fn any() -> CaseResult {
    repeat_wait(true, false)
}
fn specific_signal() -> CaseResult {
    repeat_wait(false, true)
}
fn any_signal() -> CaseResult {
    repeat_wait(true, true)
}

static SUITE: Suite = suite(
    "waitpid",
    "Child exit waits",
    &[category(
        "exit",
        "Child exit handoff",
        &[
            case(
                "cow-signal-stack",
                "SIGCHLD writes a fork-shared signal frame across two stack pages",
                cow_signal_stack,
            ),
            case(
                "specific",
                "256 immediate exits with waitpid of a specific child",
                specific,
            ),
            case("any", "256 immediate exits with waitpid of any child", any),
            case(
                "specific-signal",
                "256 specific-child waits with a caught SIGCHLD",
                specific_signal,
            ),
            case(
                "any-signal",
                "256 any-child waits with a caught SIGCHLD",
                any_signal,
            ),
        ],
    )],
);

fn main() {
    SUITE.run()
}
