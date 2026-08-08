//! Phase-2 SIGKILL teardown proof.
//!
//! Evidence is read from the fixed per-PID boot-test tally table through
//! `/proc/trace/teardown/<pid>`. The test never samples the live trace ring or
//! mutates provider enable state.

use libbreenix::io;
use libbreenix::memory;
use libbreenix::process::{
    self, fork, getpid, waitpid, wifexited, wifsignaled, wtermsig, ForkResult,
};
use libbreenix::signal::{kill, pause, sigaction, Sigaction, SIGCHLD, SIGKILL};
use std::sync::atomic::{AtomicBool, Ordering};

const EXIT_KICK_BUCKETS: u64 = 64;
const CHILD_STACK_SIZE: usize = 64 * 1024;
const CLONE_FLAGS: u64 = 0x0000_0100 | 0x0000_0400 | 0x0020_0000 | 0x0100_0000;

static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigchld_handler(_signal: i32) {
    SIGCHLD_RECEIVED.store(true, Ordering::Release);
}

fn fail(message: &str) -> ! {
    eprintln!("SIGKILL_TEARDOWN_TEST: FAIL: {message}");
    std::process::exit(1);
}

fn yield_many(count: usize) {
    for _ in 0..count {
        let _ = process::yield_now();
    }
}

#[derive(Clone, Copy, Default)]
struct Evidence {
    defer: u64,
    reclaim: u64,
    quarantine: u64,
    sgi_sent: u64,
    kick_observed: u64,
    kick_interval: u64,
    tick_period: u64,
    masked_frames_walked: u64,
    report: u64,
    bucket_published: u64,
    bucket_observed: u64,
    bucket_collision: u64,
}

fn parse_value(contents: &str, name: &str) -> u64 {
    contents
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == name)
                .then(|| value.split_whitespace().next()?.parse().ok())
                .flatten()
        })
        .unwrap_or_else(|| fail("per-PID evidence field missing"))
}

fn evidence(pid: i32) -> Evidence {
    let path = format!("/proc/trace/teardown/{pid}");
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|_| fail("could not read per-PID teardown evidence"));
    Evidence {
        defer: parse_value(&contents, "defer"),
        reclaim: parse_value(&contents, "reclaim"),
        quarantine: parse_value(&contents, "quarantine"),
        sgi_sent: parse_value(&contents, "sgi_sent"),
        kick_observed: parse_value(&contents, "kick_observed"),
        kick_interval: parse_value(&contents, "kick_interval"),
        tick_period: parse_value(&contents, "tick_period"),
        masked_frames_walked: parse_value(&contents, "masked_frames_walked"),
        report: parse_value(&contents, "report"),
        bucket_published: parse_value(&contents, "bucket_published"),
        bucket_observed: parse_value(&contents, "bucket_observed"),
        bucket_collision: parse_value(&contents, "bucket_collision"),
    }
}

fn counter(name: &str) -> u64 {
    let contents = std::fs::read_to_string("/proc/trace/counters")
        .unwrap_or_else(|_| fail("could not read trace counters"));
    parse_value(&contents, name)
}

fn wait_for_evidence(pid: i32) -> Evidence {
    let mut current = Evidence::default();
    for _ in 0..20_000 {
        current = evidence(pid);
        if current.defer == 1
            && current.reclaim == 1
            && current.quarantine == 1
            && current.sgi_sent == 1
            && current.kick_observed == 1
            && current.report == 1
        {
            return current;
        }
        let _ = process::yield_now();
    }
    eprintln!(
        "SIGKILL_TEARDOWN_TEST: evidence timeout pid={pid} defer={} reclaim={} quarantine={} sgi={} observed={} report={}",
        current.defer,
        current.reclaim,
        current.quarantine,
        current.sgi_sent,
        current.kick_observed,
        current.report
    );
    fail("timed out waiting for per-PID teardown pairing")
}

fn assert_kill_evidence(pid: i32, current: Evidence) {
    if current.defer != 1 || current.reclaim != 1 {
        fail("defer/reclaim were not paired exactly once for victim");
    }
    if current.quarantine != 1 || current.sgi_sent != 1 || current.kick_observed != 1 {
        fail("quarantine/send/observe were not attributed exactly once to victim");
    }
    if current.kick_interval == 0 || current.kick_interval >= current.tick_period {
        fail("send-to-observe interval was not strictly shorter than one tick");
    }
    if current.masked_frames_walked != 0 {
        fail("SIGKILL path walked frames while PM was held");
    }
    if current.report != 1 {
        fail("report obligation was not redeemed exactly once");
    }
    println!(
        "SIGKILL_TEARDOWN_TEST: pid={pid} defer={} reclaim={} kick_interval={} tick_period={}",
        current.defer, current.reclaim, current.kick_interval, current.tick_period
    );
}

fn wait_killed(pid: i32) {
    for _ in 0..10_000 {
        let mut status = 0;
        match waitpid(pid, &mut status, 1) {
            Ok(waited) if waited.raw() as i32 == pid => {
                if !wifsignaled(status) || wtermsig(status) != SIGKILL {
                    fail("waitpid did not reap the victim as SIGKILL (-9)");
                }
                return;
            }
            Ok(waited) if waited.raw() == 0 => {}
            Ok(_) => fail("waitpid reaped an unexpected child"),
            Err(_) => fail("waitpid returned an error for SIGKILL victim"),
        }
        let _ = process::yield_now();
    }
    fail("timed out reaping the SIGKILL victim");
}

fn wait_exited(pid: i32) {
    for _ in 0..10_000 {
        let mut status = 0;
        match waitpid(pid, &mut status, 1) {
            Ok(waited) if waited.raw() as i32 == pid => {
                if !wifexited(status) {
                    fail("SIGKILL helper did not exit normally");
                }
                return;
            }
            Ok(waited) if waited.raw() == 0 => {}
            Ok(_) => fail("waitpid reaped an unexpected helper"),
            Err(_) => fail("waitpid returned an error for SIGKILL helper"),
        }
        let _ = process::yield_now();
    }
    fail("timed out reaping the SIGKILL helper");
}

fn fork_spinning_victim() -> i32 {
    let (ready_read, ready_write) = io::pipe().unwrap_or_else(|_| fail("victim ready pipe failed"));
    match fork().unwrap_or_else(|_| fail("fork victim failed")) {
        ForkResult::Child => {
            let _ = io::close(ready_read);
            let _ = io::write(ready_write, b"R");
            let _ = io::close(ready_write);
            loop {
                core::hint::spin_loop();
            }
        }
        ForkResult::Parent(pid) => {
            let _ = io::close(ready_write);
            let mut ready = [0u8; 1];
            if io::read(ready_read, &mut ready).ok() != Some(1) {
                fail("spinning victim did not report EL0 readiness");
            }
            let _ = io::close(ready_read);
            pid.raw() as i32
        }
    }
}

fn parent_kill_and_observe_sigchld(victim: i32) {
    SIGCHLD_RECEIVED.store(false, Ordering::Release);
    let killer = match fork().unwrap_or_else(|_| fail("fork SIGKILL helper failed")) {
        ForkResult::Child => {
            // Give the parent a deterministic scheduling window to enter
            // pause(); the kill path seeds SIGCHLD before this helper returns.
            yield_many(1_024);
            if kill(victim, SIGKILL).is_err() {
                std::process::exit(91);
            }
            std::process::exit(0);
        }
        ForkResult::Parent(pid) => pid.raw() as i32,
    };

    let _ = pause();
    if !SIGCHLD_RECEIVED.swap(false, Ordering::AcqRel) {
        fail("parent pause did not return through the SIGCHLD handler");
    }
    wait_killed(victim);
    wait_exited(killer);
}

fn same_bucket_victim(first_pid: i32) -> i32 {
    loop {
        let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("candidate pipe failed"));
        let (ready_read, ready_write) =
            io::pipe().unwrap_or_else(|_| fail("candidate ready pipe failed"));
        match fork().unwrap_or_else(|_| fail("same-bucket candidate fork failed")) {
            ForkResult::Child => {
                let _ = io::close(write_fd);
                let _ = io::close(ready_read);
                let mut command = [0u8; 1];
                if io::read(read_fd, &mut command).ok() != Some(1) {
                    std::process::exit(2);
                }
                if command[0] == b'K' {
                    let _ = io::write(ready_write, b"R");
                    let _ = io::close(ready_write);
                    loop {
                        core::hint::spin_loop();
                    }
                }
                let _ = io::close(ready_write);
                std::process::exit(0);
            }
            ForkResult::Parent(pid) => {
                let _ = io::close(read_fd);
                let _ = io::close(ready_write);
                let candidate = pid.raw() as i32;
                let congruent =
                    candidate as u64 % EXIT_KICK_BUCKETS == first_pid as u64 % EXIT_KICK_BUCKETS;
                let command = if congruent { b"K" } else { b"X" };
                if io::write(write_fd, command).ok() != Some(1) {
                    fail("could not release same-bucket candidate");
                }
                let _ = io::close(write_fd);
                if congruent {
                    let mut ready = [0u8; 1];
                    if io::read(ready_read, &mut ready).ok() != Some(1) {
                        fail("same-bucket victim did not report EL0 readiness");
                    }
                    let _ = io::close(ready_read);
                    println!(
                        "SIGKILL_TEARDOWN_TEST: V1={} V2={} bucket={} congruent=true",
                        first_pid,
                        candidate,
                        candidate as u64 % EXIT_KICK_BUCKETS
                    );
                    if candidate as u64 % EXIT_KICK_BUCKETS != first_pid as u64 % EXIT_KICK_BUCKETS
                    {
                        fail("same-bucket congruence assertion failed");
                    }
                    return candidate;
                }
                let _ = io::close(ready_read);
                let mut status = 0;
                let _ = waitpid(candidate, &mut status, 0);
            }
        }
    }
}

#[repr(C)]
struct CloneShared {
    victim_pid: u64,
    sibling_heartbeat: u64,
    sibling_release: u64,
    victim_tid: u32,
    sibling_tid: u32,
}

unsafe fn thread_exit(code: u64) -> ! {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "int 0x80",
        "2:",
        "pause",
        "jmp 2b",
        in("rax") 60u64,
        in("rdi") code,
        options(noreturn),
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        "2:",
        "yield",
        "b 2b",
        in("x8") 93u64,
        in("x0") code,
        options(noreturn),
    );
}

extern "C" fn clone_victim(arg: *mut u8) -> *mut u8 {
    unsafe {
        let shared = &mut *(arg as *mut CloneShared);
        shared.victim_pid = getpid().unwrap().raw();
        loop {
            core::hint::spin_loop();
        }
    }
}

extern "C" fn clone_sibling(arg: *mut u8) -> *mut u8 {
    unsafe {
        let shared = &mut *(arg as *mut CloneShared);
        while core::ptr::read_volatile(&shared.sibling_release) == 0 {
            let next = core::ptr::read_volatile(&shared.sibling_heartbeat).wrapping_add(1);
            core::ptr::write_volatile(&mut shared.sibling_heartbeat, next);
        }
        thread_exit(0);
    }
}

unsafe fn raw_clone(
    function: extern "C" fn(*mut u8) -> *mut u8,
    argument: *mut u8,
    stack_top: usize,
    child_tid: *mut u32,
) -> i64 {
    let result: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "int 0x80",
        in("rax") 56u64,
        in("rdi") CLONE_FLAGS,
        in("rsi") stack_top as u64,
        in("rdx") function as u64,
        in("r10") argument as u64,
        in("r8") child_tid as u64,
        lateout("rax") result,
        options(nostack),
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") 220u64,
        inlateout("x0") CLONE_FLAGS => result,
        in("x1") stack_top as u64,
        in("x2") function as u64,
        in("x3") argument as u64,
        in("x4") child_tid as u64,
        options(nostack),
    );
    result
}

fn clone_vm_sibling_survives() {
    unsafe {
        let shared = memory::mmap(core::ptr::null_mut(), 4096, 3, 0x22, -1, 0)
            .unwrap_or_else(|_| fail("clone shared mmap failed"))
            as *mut CloneShared;
        core::ptr::write(
            shared,
            CloneShared {
                victim_pid: 0,
                sibling_heartbeat: 0,
                sibling_release: 0,
                victim_tid: u32::MAX,
                sibling_tid: u32::MAX,
            },
        );
        let victim_stack = memory::mmap(core::ptr::null_mut(), CHILD_STACK_SIZE, 3, 0x22, -1, 0)
            .unwrap_or_else(|_| fail("clone victim stack mmap failed"));
        let sibling_stack = memory::mmap(core::ptr::null_mut(), CHILD_STACK_SIZE, 3, 0x22, -1, 0)
            .unwrap_or_else(|_| fail("clone sibling stack mmap failed"));

        if raw_clone(
            clone_victim,
            shared.cast(),
            (victim_stack as usize + CHILD_STACK_SIZE) & !0xf,
            &mut (*shared).victim_tid,
        ) < 0
            || raw_clone(
                clone_sibling,
                shared.cast(),
                (sibling_stack as usize + CHILD_STACK_SIZE) & !0xf,
                &mut (*shared).sibling_tid,
            ) < 0
        {
            fail("CLONE_VM setup failed");
        }

        for _ in 0..2_000_000 {
            if core::ptr::read_volatile(&(*shared).victim_pid) != 0
                && core::ptr::read_volatile(&(*shared).sibling_heartbeat) != 0
            {
                break;
            }
            let _ = process::yield_now();
        }
        let victim = core::ptr::read_volatile(&(*shared).victim_pid) as i32;
        if victim == 0 {
            fail("CLONE_VM victim did not publish its PID");
        }
        let _before = evidence(victim);
        let heartbeat_before = core::ptr::read_volatile(&(*shared).sibling_heartbeat);
        kill(victim, SIGKILL).unwrap_or_else(|_| fail("CLONE_VM victim kill failed"));
        wait_killed(victim);
        yield_many(64);
        let heartbeat_after = core::ptr::read_volatile(&(*shared).sibling_heartbeat);
        if heartbeat_after <= heartbeat_before {
            fail("CLONE_VM sibling did not survive victim-only SIGKILL");
        }
        assert_kill_evidence(victim, wait_for_evidence(victim));
        core::ptr::write_volatile(&mut (*shared).sibling_release, 1);
        // Sibling survival is the P2 invariant. CLONE_CHILD_CLEARTID wake/clear
        // semantics are not part of this gate, so only give the released sibling
        // a scheduling window rather than treating that later ABI as evidence.
        yield_many(64);
    }
}

fn self_kill_child() {
    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("self-kill pipe failed"));
    match fork().unwrap_or_else(|_| fail("self-kill fork failed")) {
        ForkResult::Child => {
            let _ = io::close(write_fd);
            let mut release = [0u8; 1];
            if io::read(read_fd, &mut release).ok() != Some(1) {
                std::process::exit(98);
            }
            let own_pid = getpid().unwrap().raw() as i32;
            println!("SIGKILL_TEARDOWN_TEST: self-killing pid={own_pid}");
            let _ = kill(own_pid, SIGKILL);
            std::process::exit(99);
        }
        ForkResult::Parent(pid) => {
            let _ = io::close(read_fd);
            let raw_pid = pid.raw() as i32;
            println!("SIGKILL_TEARDOWN_TEST: awaiting self-kill pid={raw_pid}");
            let _before = evidence(raw_pid);
            if io::write(write_fd, b"K").ok() != Some(1) {
                fail("could not release self-kill child");
            }
            let _ = io::close(write_fd);
            wait_killed(raw_pid);
            println!("SIGKILL_TEARDOWN_TEST: self-kill waitpid reaped");
            assert_kill_evidence(raw_pid, wait_for_evidence(raw_pid));
        }
    }
}

fn main() {
    sigaction(SIGCHLD, Some(&Sigaction::new(sigchld_handler)), None)
        .unwrap_or_else(|_| fail("SIGCHLD sigaction failed"));

    let fault_entry_before = counter("TEARDOWN_ENTRY_FAULT");
    let cr3_miss_before = counter("TEARDOWN_CR3_MISS");
    let attribution_uncertain_before = counter("EXIT_ATTRIBUTION_UNCERTAIN");
    let published_before = counter("EXIT_KICK_PUBLISHED");
    let observed_before = counter("EXIT_KICK_OBSERVED");
    let collision_before = counter("EXIT_KICK_BUCKET_COLLISION");

    let victim_one = fork_spinning_victim();
    let bucket_before = evidence(victim_one);
    parent_kill_and_observe_sigchld(victim_one);
    let first = wait_for_evidence(victim_one);
    assert_kill_evidence(victim_one, first);
    if first.bucket_published - bucket_before.bucket_published != 1
        || first.bucket_observed - bucket_before.bucket_observed != 1
        || first.bucket_collision - bucket_before.bucket_collision != 0
        || counter("EXIT_KICK_PUBLISHED") - published_before != 1
        || counter("EXIT_KICK_OBSERVED") - observed_before != 1
        || counter("EXIT_KICK_BUCKET_COLLISION") - collision_before != 0
    {
        fail("single-victim kick accounting was not exactly one-to-one");
    }

    let victim_two = same_bucket_victim(victim_one);
    let second_before = evidence(victim_two);
    yield_many(32);
    println!("SIGKILL_TEARDOWN_TEST: killing V2={victim_two}");
    kill(victim_two, SIGKILL).unwrap_or_else(|_| fail("second victim kill failed"));
    println!("SIGKILL_TEARDOWN_TEST: V2 kill returned");
    wait_killed(victim_two);
    println!("SIGKILL_TEARDOWN_TEST: V2 waitpid reaped");
    let second = wait_for_evidence(victim_two);
    assert_kill_evidence(victim_two, second);
    if victim_one as u64 % EXIT_KICK_BUCKETS != victim_two as u64 % EXIT_KICK_BUCKETS
        || second.bucket_published - bucket_before.bucket_published != 2
        || second.bucket_observed - bucket_before.bucket_observed != 2
        || second.bucket_collision - bucket_before.bucket_collision != 0
        || second.bucket_published - second_before.bucket_published != 1
        || second.bucket_observed - second_before.bucket_observed != 1
        || counter("EXIT_KICK_PUBLISHED") - published_before != 2
        || counter("EXIT_KICK_OBSERVED") - observed_before != 2
        || counter("EXIT_KICK_BUCKET_COLLISION") - collision_before != 0
    {
        fail("sequential same-bucket reuse accounting failed");
    }

    clone_vm_sibling_survives();
    println!("SIGKILL_TEARDOWN_TEST: CLONE_VM sibling survived");
    self_kill_child();

    if counter("RECLAIM_ENQUEUE_UNDER_PM") != 0
        || counter("TEARDOWN_ENTRY_FAULT") != fault_entry_before
        || counter("TEARDOWN_CR3_MISS") != cr3_miss_before
        || counter("EXIT_ATTRIBUTION_UNCERTAIN") != attribution_uncertain_before
        || counter("EXIT_REQUEST_OBSERVED") != 0
        || counter("RECEIPT_DROPPED_UNRETIRED") != 0
        || counter("LEDGER_CLAIM_MISMATCH") != 0
        || counter("LEDGER_CLAIM_ORPHANED") != 0
    {
        fail("P2 zero-only lock-order, fault, receipt, or later-phase counters moved");
    }

    println!("SIGKILL_TEARDOWN_TEST_PASSED");
}
