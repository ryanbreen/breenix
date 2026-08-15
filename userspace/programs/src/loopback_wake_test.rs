//! Event-driven userspace regression test for issue #545, where a reader
//! blocked in TCP `recv()` on an x86 loopback connection could remain asleep
//! forever.
//!
//! Before opening the FIN window, the parent waits for `/proc/pids` to contain
//! only itself and for the observed PID high-water mark to remain stable. This
//! prevents another userspace test's network syscall from draining this test's
//! queued FIN. Ordering remains pipe-carried,
//! and the peer embeds its pre-write timestamp so the reader measures true data
//! delivery latency; neither operation drains loopback work.
//!
//! The peer exits without closing its socket, so process teardown emits the
//! undrained FIN under test. During that window, a calibrated load child spins
//! for about 16 seconds without making a syscall, which keeps the idle drain
//! from running. With `kloopbackd`, the blocking reader sees EOF within its
//! 6-second bound. Without the pump, the FIN cannot be delivered until the load
//! child stops and the CPU becomes idle, so the EOF check fails; if the idle
//! drain is also absent, the watchdog is the sole bounded escape.

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::process::{fork, getpid, waitpid, wexitstatus, wifexited, ForkResult};
use libbreenix::signal;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::time::{now_monotonic, sleep_ms};
use libbreenix::types::{Fd, Pid};
use std::process as std_process;

const LISTEN_PORT: u16 = 54530;
const TAG: &[u8] = b"545-wake";
const PAYLOAD_LEN: usize = 16;
const QUIESCE_POLL_MS: u64 = 250;
const QUIESCE_STABLE_MS: u64 = 5000;
const QUIESCE_BOUND_MS: u64 = 300_000;
const CALIBRATION_ITERS: u64 = 5_000_000;
const DATA_WAKE_BOUND_MS: u64 = 4000;
const EOF_WAKE_BOUND_MS: u64 = 6000;
const LOAD_SPIN_MIN_MS: u64 = 9000;
// Post-fork calibration can underestimate by ~44%; this target still leaves the
// 9000 ms floor 3000 ms above EOF_WAKE_BOUND_MS.
const LOAD_SPIN_MS: u64 = 16000;
const WATCHDOG_AT_MS: u64 = 60000;

fn monotonic_ms() -> Option<u64> {
    let now = now_monotonic().ok()?;
    Some(
        (now.tv_sec.max(0) as u64)
            .saturating_mul(1000)
            .saturating_add((now.tv_nsec.max(0) as u64) / 1_000_000),
    )
}

fn role_now_or_exit(exit_code: i32) -> u64 {
    match monotonic_ms() {
        Some(now) => now,
        None => std_process::exit(exit_code),
    }
}

fn wait_for_quiescence() {
    let parent_pid = match getpid() {
        Ok(pid) => pid,
        Err(_) => {
            println!("LOOPBACK_WAKE_TEST: quiesce waited_ms=0 others=0 quiesced=0");
            return;
        }
    };
    let start_ms = match monotonic_ms() {
        Some(now) => now,
        None => {
            println!("LOOPBACK_WAKE_TEST: quiesce waited_ms=0 others=0 quiesced=0");
            return;
        }
    };
    let mut largest_pid_seen = parent_pid.raw();
    let mut largest_pid_stable_since_ms = start_ms;
    let mut waited_ms = 0;
    let mut others = 0usize;
    let mut quiesced = false;

    loop {
        let now_ms = match monotonic_ms() {
            Some(now) => now,
            None => break,
        };
        waited_ms = now_ms.saturating_sub(start_ms);

        match std::fs::read_to_string("/proc/pids") {
            Ok(contents) => {
                let mut listed = 0usize;
                let mut saw_parent = false;
                let mut largest_pid = 0u64;
                others = 0;

                for pid in contents.lines().filter_map(|line| line.parse::<u64>().ok()) {
                    listed = listed.saturating_add(1);
                    largest_pid = largest_pid.max(pid);
                    if pid == parent_pid.raw() {
                        saw_parent = true;
                    } else {
                        others = others.saturating_add(1);
                    }
                }

                if largest_pid > largest_pid_seen {
                    largest_pid_seen = largest_pid;
                    largest_pid_stable_since_ms = now_ms;
                }
                let only_parent = listed == 1 && saw_parent && others == 0;
                let high_water_stable =
                    now_ms.saturating_sub(largest_pid_stable_since_ms) >= QUIESCE_STABLE_MS;
                if only_parent && high_water_stable {
                    quiesced = true;
                    break;
                }
                if listed == 0 {
                    largest_pid_stable_since_ms = now_ms;
                }
            }
            Err(_) => largest_pid_stable_since_ms = now_ms,
        }

        let remaining_ms = QUIESCE_BOUND_MS.saturating_sub(waited_ms);
        if remaining_ms == 0 {
            break;
        }
        if sleep_ms(remaining_ms.min(QUIESCE_POLL_MS)).is_err() {
            core::hint::spin_loop();
        }
    }

    println!(
        "LOOPBACK_WAKE_TEST: quiesce waited_ms={} others={} quiesced={}",
        waited_ms,
        others,
        u8::from(quiesced)
    );
}

fn watchdog_sleep_until(epoch_ms: u64) {
    let target_ms = epoch_ms.saturating_add(WATCHDOG_AT_MS);
    loop {
        let now_ms = role_now_or_exit(40);
        let remaining_ms = target_ms.saturating_sub(now_ms);
        if remaining_ms == 0 {
            return;
        }
        if sleep_ms(remaining_ms).is_err() {
            core::hint::spin_loop();
        }
    }
}

fn reader_child(server_fd: Fd, ready_w: Fd) -> ! {
    let connection = match socket::accept(server_fd, None) {
        Ok(fd) => fd,
        Err(_) => std_process::exit(10),
    };

    let mut buffer = [0u8; PAYLOAD_LEN];
    let data_bytes = match io::read(connection, &mut buffer) {
        Ok(bytes) => bytes,
        Err(_) => std_process::exit(11),
    };
    let t_data = role_now_or_exit(16);
    if data_bytes != PAYLOAD_LEN || &buffer[8..] != TAG {
        std_process::exit(12);
    }

    let mut peer_write_bytes = [0u8; 8];
    peer_write_bytes.copy_from_slice(&buffer[..8]);
    let peer_write_ms = u64::from_le_bytes(peer_write_bytes);
    let data_latency_ms = t_data.saturating_sub(peer_write_ms);
    println!(
        "LOOPBACK_WAKE_TEST: data latency_ms={} bytes={}",
        data_latency_ms, data_bytes
    );
    if data_latency_ms > DATA_WAKE_BOUND_MS {
        std_process::exit(13);
    }

    match io::write(ready_w, b"g") {
        Ok(1) => {}
        _ => std_process::exit(17),
    }
    let t_ready = role_now_or_exit(16);
    let eof_result = io::read(connection, &mut buffer);
    let t_eof = role_now_or_exit(16);
    let eof_wait_ms = t_eof.saturating_sub(t_ready);
    let eof_bytes = eof_result.as_ref().copied().unwrap_or(0);
    println!(
        "LOOPBACK_WAKE_TEST: eof wait_ms={} bytes={}",
        eof_wait_ms, eof_bytes
    );
    if !matches!(eof_result, Ok(0)) {
        std_process::exit(14);
    }
    if eof_wait_ms > EOF_WAKE_BOUND_MS {
        std_process::exit(15);
    }

    std_process::exit(0);
}

fn peer_child(spin_r: Fd) -> ! {
    let connection = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => std_process::exit(20),
    };
    let server_addr = SockAddrIn::new([127, 0, 0, 1], LISTEN_PORT);
    if socket::connect_inet(connection, &server_addr).is_err() {
        std_process::exit(21);
    }

    let mut payload = [0u8; PAYLOAD_LEN];
    payload[8..].copy_from_slice(TAG);
    let peer_write_ms = role_now_or_exit(23);
    payload[..8].copy_from_slice(&peer_write_ms.to_le_bytes());
    match io::write(connection, &payload) {
        Ok(PAYLOAD_LEN) => {}
        _ => std_process::exit(22),
    }

    let mut spin_signal = [0u8; 1];
    match io::read(spin_r, &mut spin_signal) {
        Ok(0) => std_process::exit(24),
        Ok(_) => {}
        Err(_) => std_process::exit(25),
    }

    // Do not close the TCP socket: process teardown must emit the FIN because
    // nothing on the teardown path drains the loopback queue.
    std_process::exit(0);
}

#[inline(never)]
fn spin_iterations(iterations: u64) {
    let mut remaining = iterations;
    while remaining != 0 {
        core::hint::spin_loop();
        remaining -= 1;
    }
}

fn load_child(ready_r: Fd, spin_w: Fd) -> ! {
    let calibration_start_ms = role_now_or_exit(30);
    spin_iterations(CALIBRATION_ITERS);
    let calibration_elapsed_ms = role_now_or_exit(30).saturating_sub(calibration_start_ms);
    let iterations_per_ms = (CALIBRATION_ITERS / calibration_elapsed_ms.max(1)).max(1);
    let load_iterations = iterations_per_ms.saturating_mul(LOAD_SPIN_MS);

    let mut ready_signal = [0u8; 1];
    match io::read(ready_r, &mut ready_signal) {
        Ok(0) => std_process::exit(31),
        Ok(_) => {}
        Err(_) => std_process::exit(32),
    }

    let spin_start_ms = role_now_or_exit(30);
    match io::write(spin_w, b"s") {
        Ok(1) => {}
        _ => std_process::exit(33),
    }

    // No syscall-bearing operation may appear between the spin signal and the
    // end of this call: the load must deny idle_thread_fn the FIN window.
    spin_iterations(load_iterations);

    let measured_spin_ms = role_now_or_exit(30).saturating_sub(spin_start_ms);
    println!("LOOPBACK_WAKE_TEST: load spin_ms={}", measured_spin_ms);
    if measured_spin_ms < LOAD_SPIN_MIN_MS {
        std_process::exit(34);
    }

    std_process::exit(0);
}

fn watchdog_child(epoch_ms: u64, reader_pid: Pid, peer_pid: Pid, load_pid: Pid) -> ! {
    watchdog_sleep_until(epoch_ms);
    // This is the only bounded escape if a wake is lost so completely that a
    // role never returns from a blocking read. ESRCH is normal: all siblings
    // should be long gone before the watchdog fires.
    let _ = signal::kill(reader_pid.raw() as i32, 9);
    let _ = signal::kill(peer_pid.raw() as i32, 9);
    let _ = signal::kill(load_pid.raw() as i32, 9);
    std_process::exit(0);
}

fn wait_status(pid: Pid) -> Option<i32> {
    let mut status = 0;
    loop {
        match waitpid(pid.raw() as i32, &mut status, 0) {
            Ok(reaped) if reaped == pid => return Some(status),
            Err(Error::Os(Errno::EINTR)) => {
                // A sibling's SIGCHLD can interrupt this specific-pid wait.
                // The watchdog, not this loop, bounds a child that never exits.
                continue;
            }
            Ok(_) | Err(_) => return None,
        }
    }
}

fn child_failure_reason(role: &str, status: Option<i32>) -> Option<String> {
    match status {
        Some(status) if wifexited(status) && wexitstatus(status) == 0 => None,
        Some(status) if wifexited(status) => Some(format!("{}_exit_{}", role, wexitstatus(status))),
        Some(status) => Some(format!("{}_signal_{}", role, status & 0x7f)),
        None => Some(format!("{}_wait", role)),
    }
}

fn fail(reason: &str) -> ! {
    println!("[TEST:userspace:loopback_recv_wake:FAIL:{}]", reason);
    std_process::exit(1);
}

fn main() {
    println!("[TEST:userspace:loopback_recv_wake:START]");

    let server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket"),
    };
    let listen_addr = SockAddrIn::new([0, 0, 0, 0], LISTEN_PORT);
    if socket::bind_inet(server_fd, &listen_addr).is_err() {
        fail("bind");
    }
    if socket::listen(server_fd, 4).is_err() {
        fail("listen");
    }
    // No process explicitly closes the listener.

    let (ready_r, ready_w) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(_) => fail("ready_pipe"),
    };
    let (spin_r, spin_w) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(_) => fail("spin_pipe"),
    };

    wait_for_quiescence();

    // Only the watchdog uses this epoch, as an outer bound on a total hang.
    let epoch_ms = match monotonic_ms() {
        Some(now) => now,
        None => fail("clock"),
    };

    // No process closes any pipe end. The parent deliberately keeps every end
    // open so a dead sibling cannot turn a blocking synchronization read into
    // spurious EOF; the watchdog is the single bounded escape from any hang.
    let reader_pid = match fork() {
        Ok(ForkResult::Child) => reader_child(server_fd, ready_w),
        Ok(ForkResult::Parent(pid)) => pid,
        Err(_) => fail("reader_fork"),
    };
    let peer_pid = match fork() {
        Ok(ForkResult::Child) => peer_child(spin_r),
        Ok(ForkResult::Parent(pid)) => pid,
        Err(_) => {
            let _ = signal::kill(reader_pid.raw() as i32, 9);
            fail("peer_fork");
        }
    };
    let load_pid = match fork() {
        Ok(ForkResult::Child) => load_child(ready_r, spin_w),
        Ok(ForkResult::Parent(pid)) => pid,
        Err(_) => {
            let _ = signal::kill(reader_pid.raw() as i32, 9);
            let _ = signal::kill(peer_pid.raw() as i32, 9);
            fail("load_fork");
        }
    };
    let watchdog_pid = match fork() {
        Ok(ForkResult::Child) => watchdog_child(epoch_ms, reader_pid, peer_pid, load_pid),
        Ok(ForkResult::Parent(pid)) => pid,
        Err(_) => {
            let _ = signal::kill(reader_pid.raw() as i32, 9);
            let _ = signal::kill(peer_pid.raw() as i32, 9);
            let _ = signal::kill(load_pid.raw() as i32, 9);
            fail("watchdog_fork");
        }
    };

    // The parent stays blocked in waitpid throughout the test; it never polls.
    let reader_status = wait_status(reader_pid);
    let peer_status = wait_status(peer_pid);
    let load_status = wait_status(load_pid);
    let watchdog_status = wait_status(watchdog_pid);

    let failure = child_failure_reason("reader", reader_status)
        .or_else(|| child_failure_reason("peer", peer_status))
        .or_else(|| child_failure_reason("load", load_status))
        .or_else(|| child_failure_reason("watchdog", watchdog_status));

    if let Some(reason) = failure {
        fail(&reason);
    }

    println!("[TEST:userspace:loopback_recv_wake:PASS]");
    std_process::exit(0);
}
