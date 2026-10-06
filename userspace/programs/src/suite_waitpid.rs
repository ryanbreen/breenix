//! Blocking and nonblocking child waits, followed by a fork-shared signal stack.
use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::memory::{self, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::signal::{self, Sigaction, StackT, SA_ONSTACK, SA_RESTART, SIGCHLD};
use libbreenix::suite::{case, category, check, suite, CaseResult, Suite};
use libbreenix::time;
use std::sync::atomic::{AtomicUsize, Ordering};

static SIGNALS: AtomicUsize = AtomicUsize::new(0);
static HANDLER_STACK: AtomicUsize = AtomicUsize::new(0);

extern "C" fn child_exited(_: i32) {
    let local = 0u8;
    // Taking a volatile address ensures this is storage on the handler's stack.
    unsafe { core::ptr::read_volatile(&local) };
    HANDLER_STACK.store(core::ptr::addr_of!(local) as usize, Ordering::Relaxed);
    SIGNALS.fetch_add(1, Ordering::Relaxed);
}

fn cow_signal_stack() -> CaseResult {
    let stack = memory::mmap(
        core::ptr::null_mut(), 16384, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0,
    )?;
    // Populate before fork; the parent leaves these pages untouched until delivery.
    // The top sits 128 bytes into a page, so the saved frame crosses two pages.
    unsafe { core::ptr::write_bytes(stack, 0xa5, 16384) };
    let alt = StackT { ss_sp: stack as u64, ss_flags: 0, _pad: 0, ss_size: 8320 };
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
            check(process::wifexited(status) && process::wexitstatus(status) == 42,
                "CoW signal stack: wrong exit status")?;
            check(SIGNALS.load(Ordering::Relaxed) == 1, "CoW signal stack: missing handler")?;
            let local = HANDLER_STACK.load(Ordering::Relaxed);
            check(local >= stack as usize && local < stack as usize + 8320,
                "handler did not run on the alternate stack")?;
            // Validate the installed frame itself, including the saved mask on
            // the second page. These sizes are the architecture's signal ABI.
            #[cfg(target_arch = "aarch64")]
            let frame_size = 320;
            #[cfg(target_arch = "x86_64")]
            let frame_size = 192;
            let frame = (stack as usize + 8320 - frame_size) & !15;
            check(frame / 4096 != (frame + frame_size - 1) / 4096,
                "signal frame did not span two pages")?;
            check(unsafe { core::ptr::read_volatile((frame + 8) as *const u64) }
                == 0xDEAD_BEEF_CAFE_BABE, "signal frame magic missing")?;
            check(unsafe { core::ptr::read_volatile((frame + frame_size - 8) as *const u64) }
                == 0, "saved signal mask missing on second page")?;
        }
    }
    signal::sigaltstack(Some(&StackT::default()), None)?;
    memory::munmap(stack, 16384)?;
    Ok(())
}

fn repeat_wait(any_child: bool, caught: bool, delayed: bool) -> CaseResult {
    if caught {
        let mut action = Sigaction::new(child_exited);
        action.flags |= SA_RESTART;
        signal::sigaction(SIGCHLD, Some(&action), None)?;
    }
    let count = if delayed { 1 } else { 256 };
    for iteration in 0..count {
        match process::fork()? {
            ForkResult::Child => {
                if delayed { time::sleep_ms(20)?; }
                process::exit(42)
            }
            ForkResult::Parent(child) => {
                let mut status = 0;
                let waited = process::waitpid(
                    if any_child { -1 } else { child.raw() as i32 }, &mut status, 0,
                )?;
                check(waited == child, &format!("iteration {iteration}: wrong child"))?;
                check(process::wifexited(status) && process::wexitstatus(status) == 42,
                    &format!("iteration {iteration}: wrong exit status"))?;
                if caught {
                    check(SIGNALS.load(Ordering::Relaxed) == iteration + 1,
                        &format!("iteration {iteration}: missing SIGCHLD"))?;
                }
            }
        }
    }
    Ok(())
}

fn specific() -> CaseResult { repeat_wait(false, false, false) }
fn any() -> CaseResult { repeat_wait(true, false, false) }
fn specific_signal() -> CaseResult { repeat_wait(false, true, false) }
fn any_signal() -> CaseResult { repeat_wait(true, true, false) }
fn parked_specific() -> CaseResult { repeat_wait(false, false, true) }
fn parked_any() -> CaseResult { repeat_wait(true, false, true) }
fn parked_specific_signal() -> CaseResult { repeat_wait(false, true, true) }
fn parked_any_signal() -> CaseResult { repeat_wait(true, true, true) }

fn nohang() -> CaseResult {
    match process::fork()? {
        ForkResult::Child => { time::sleep_ms(20)?; process::exit(42) }
        ForkResult::Parent(child) => {
            let mut status = 0;
            check(process::waitpid(child.raw() as i32, &mut status, WNOHANG)?.raw() == 0,
                "WNOHANG returned a running child")?;
            let start = time::now_monotonic()?.as_nanos();
            loop {
                let waited = process::waitpid(child.raw() as i32, &mut status, WNOHANG)?;
                if waited.raw() != 0 {
                    check(waited == child && process::wifexited(status)
                        && process::wexitstatus(status) == 42, "WNOHANG returned wrong status")?;
                    break;
                }
                check(time::now_monotonic()?.as_nanos() - start < 1_000_000_000,
                    "WNOHANG never reaped exited child")?;
                process::yield_now()?;
            }
        }
    }
    Ok(())
}

fn interrupted() -> CaseResult {
    signal::sigaction(SIGCHLD, Some(&Sigaction::new(child_exited)), None)?;
    let parent = process::getpid()?;
    match process::fork()? {
        ForkResult::Child => {
            time::sleep_ms(20)?;
            signal::kill(parent.raw() as i32, SIGCHLD)?;
            time::sleep_ms(50)?;
            process::exit(42)
        }
        ForkResult::Parent(child) => {
            let mut status = 0;
            check(matches!(process::waitpid(child.raw() as i32, &mut status, 0),
                Err(Error::Os(Errno::EINTR))), "caught SIGCHLD did not interrupt waitpid")?;
            check(SIGNALS.load(Ordering::Relaxed) == 1, "interrupting handler missing")?;
            let waited = process::waitpid(child.raw() as i32, &mut status, 0)?;
            check(waited == child && process::wifexited(status)
                && process::wexitstatus(status) == 42, "wait after EINTR failed")?;
        }
    }
    Ok(())
}

fn stack_trampoline() -> CaseResult {
    let mut action = Sigaction::new_without_restorer(child_exited);
    action.flags |= SA_RESTART;
    signal::sigaction(SIGCHLD, Some(&action), None)?;
    match process::fork()? {
        ForkResult::Child => process::exit(42),
        ForkResult::Parent(child) => {
            let mut status = 0;
            check(process::waitpid(child.raw() as i32, &mut status, 0)? == child,
                "stack trampoline wait failed")?;
            check(SIGNALS.load(Ordering::Relaxed) == 1 && process::wifexited(status)
                && process::wexitstatus(status) == 42, "stack trampoline did not return")?;
        }
    }
    Ok(())
}

static SUITE: Suite = suite(
    "waitpid", "Child exit waits", &[category(
        "exit", "Child exit handoff", &[
            case("specific", "256 immediate exits with waitpid of a specific child", specific),
            case("any", "256 immediate exits with waitpid of any child", any),
            case("specific-signal", "256 specific-child waits with a caught SIGCHLD", specific_signal),
            case("any-signal", "256 any-child waits with a caught SIGCHLD", any_signal),
            case("parked-specific", "Specific-child wait before a delayed exit", parked_specific),
            case("parked-any", "Any-child wait before a delayed exit", parked_any),
            case("parked-specific-signal", "Specific-child wait with SIGCHLD before a delayed exit", parked_specific_signal),
            case("parked-any-signal", "Any-child wait with SIGCHLD before a delayed exit", parked_any_signal),
            case("nohang", "WNOHANG returns zero before exit and the PID afterward", nohang),
            case("interrupted", "Caught SIGCHLD without SA_RESTART interrupts waitpid", interrupted),
            case("stack-trampoline", "SIGCHLD returns through the stack trampoline", stack_trampoline),
            case("cow-signal-stack", "SIGCHLD writes a fork-shared signal frame across two stack pages", cow_signal_stack),
        ],
    )],
);

fn main() { SUITE.run() }
