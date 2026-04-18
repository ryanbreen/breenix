//! F32c waitqueue race reproducer.
//!
//! This program intentionally stresses a waitqueue with no persistent condition:
//! a wake that lands after queue enrollment must still prevent the waiter from
//! sleeping. Linux closes that race by setting task state under the waitqueue
//! lock and making schedule a no-op for TASK_RUNNING tasks.

use libbreenix::graphics;
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::signal::{kill, SIGKILL};
use libbreenix::time;
use libbreenix::types::Timespec;
use std::process as std_process;

const DEFAULT_DURATION_SECS: u64 = 60;
const SAMPLE_MS: u64 = 100;

#[derive(Clone, Copy, Debug, Default)]
struct Stats {
    entered: u64,
    returned: u64,
    wakes: u64,
    has_waiters: u64,
}

fn read_stats() -> Stats {
    let mut raw = [0u64; 4];
    if let Err(err) = graphics::wait_stress_stats(&mut raw) {
        println!("WAIT_STRESS_ERROR stats failed: {:?}", err);
        std_process::exit(2);
    }
    Stats {
        entered: raw[0],
        returned: raw[1],
        wakes: raw[2],
        has_waiters: raw[3],
    }
}

fn sleep_ms(ms: u64) {
    let ts = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    let _ = time::nanosleep(&ts);
}

fn parse_duration_secs() -> u64 {
    std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_DURATION_SECS)
}

fn waiter() -> ! {
    loop {
        if let Err(err) = graphics::wait_stress_wait() {
            println!("WAIT_STRESS_ERROR waiter failed: {:?}", err);
            std_process::exit(10);
        }
    }
}

fn waker() -> ! {
    let mut wakes = 0u64;
    loop {
        if let Err(err) = graphics::wait_stress_wake() {
            println!("WAIT_STRESS_ERROR waker failed: {:?}", err);
            std_process::exit(11);
        }
        wakes += 1;
        if wakes & 0x3f == 0 {
            let _ = process::yield_now();
        }
    }
}

fn fork_child(role: &str, f: fn() -> !) -> u64 {
    match process::fork() {
        Ok(ForkResult::Child) => f(),
        Ok(ForkResult::Parent(pid)) => {
            println!("WAIT_STRESS: forked {} pid={}", role, pid.raw());
            pid.raw()
        }
        Err(err) => {
            println!("WAIT_STRESS_ERROR fork {} failed: {:?}", role, err);
            std_process::exit(3);
        }
    }
}

fn cleanup(waiter_pid: u64, waker_pid: u64) {
    let _ = kill(waiter_pid as i32, SIGKILL);
    let _ = kill(waker_pid as i32, SIGKILL);

    let mut status = 0i32;
    let mut waiter_reaped = false;
    let mut waker_reaped = false;
    for _ in 0..20 {
        if !waiter_reaped
            && process::waitpid(waiter_pid as i32, &mut status, WNOHANG)
                .map(|pid| pid.raw() != 0)
                .unwrap_or(false)
        {
            waiter_reaped = true;
        }
        if !waker_reaped
            && process::waitpid(waker_pid as i32, &mut status, WNOHANG)
                .map(|pid| pid.raw() != 0)
                .unwrap_or(false)
        {
            waker_reaped = true;
        }
        if waiter_reaped && waker_reaped {
            break;
        }
        sleep_ms(10);
    }
}

fn main() {
    let duration_secs = parse_duration_secs();
    println!(
        "WAIT_STRESS_START duration={}s sample={}ms",
        duration_secs, SAMPLE_MS
    );

    if let Err(err) = graphics::wait_stress_reset() {
        println!("WAIT_STRESS_ERROR reset failed: {:?}", err);
        std_process::exit(1);
    }

    let waiter_pid = fork_child("waiter", waiter);
    let waker_pid = fork_child("waker", waker);

    let samples = (duration_secs * 1000).div_ceil(SAMPLE_MS);
    let mut last = read_stats();

    for sample in 0..samples {
        sleep_ms(SAMPLE_MS);
        let stats = read_stats();

        if stats.entered > stats.returned
            && stats.returned == last.returned
            && stats.wakes > last.wakes
        {
            println!(
                "WAIT_STRESS_STALL sample={} entered={} returned={} wakes={} waiters={}",
                sample + 1,
                stats.entered,
                stats.returned,
                stats.wakes,
                stats.has_waiters
            );
            cleanup(waiter_pid, waker_pid);
            std_process::exit(4);
        }

        if (sample + 1) % 10 == 0 {
            println!(
                "WAIT_STRESS_PROGRESS sample={} entered={} returned={} wakes={} waiters={}",
                sample + 1,
                stats.entered,
                stats.returned,
                stats.wakes,
                stats.has_waiters
            );
        }

        last = stats;
    }

    let final_stats = read_stats();
    cleanup(waiter_pid, waker_pid);
    println!(
        "WAIT_STRESS_PASS entered={} returned={} wakes={} waiters={}",
        final_stats.entered, final_stats.returned, final_stats.wakes, final_stats.has_waiters
    );
    std_process::exit(0);
}
