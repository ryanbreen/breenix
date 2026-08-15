//! Userspace regression test for issue #545, where a reader blocked in TCP
//! `recv()` on an x86 loopback connection could remain asleep forever.
//!
//! A userspace write drains its own loopback segment, so the first read only
//! establishes that the connection works. The peer then exits without closing
//! its socket; process teardown emits an undrained FIN that only the branch's
//! loopback delivery mechanisms can deliver. A load child keeps the CPU busy
//! across that FIN-delivery window so `kloopbackd`, rather than the idle-loop
//! drain, must deliver it. A lost wake is reported as exit 15 when delivery
//! exceeds the bound, or as the watchdog killing a reader that never wakes.

use libbreenix::io;
use libbreenix::process::{fork, waitpid, wifexited, wexitstatus, ForkResult};
use libbreenix::signal;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::time::{now_monotonic, sleep_ms};
use libbreenix::types::{Fd, Pid};
use std::process as std_process;

const LISTEN_PORT: u16 = 54530;
const PAYLOAD: &[u8] = b"545-wake";
const PEER_WRITE_AT_MS: u64 = 600;
const LOAD_START_AT_MS: u64 = 1400;
const PEER_EXIT_AT_MS: u64 = 1600;
const DATA_WAKE_BOUND_MS: u64 = 2000;
const EOF_WAKE_BOUND_MS: u64 = 2000;
// The EOF deadline is epoch + 1600 + 2000 = 3600 ms. A kernel that loses
// loopback delivery liveness cannot deliver the FIN until load ends at epoch
// + 6400 ms, leaving 2800 ms of separation; keep the load window beyond it.
const LOAD_END_AT_MS: u64 = 6400;
const WATCHDOG_AT_MS: u64 = 12000;

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

fn sleep_until(epoch_ms: u64, offset_ms: u64, clock_error_exit: i32) {
    let target_ms = epoch_ms.saturating_add(offset_ms);
    loop {
        let now_ms = role_now_or_exit(clock_error_exit);
        let remaining_ms = target_ms.saturating_sub(now_ms);
        if remaining_ms == 0 {
            return;
        }
        if sleep_ms(remaining_ms).is_err() {
            core::hint::spin_loop();
        }
    }
}

fn reader_child(server_fd: Fd, epoch_ms: u64) -> ! {
    let connection = match socket::accept(server_fd, None) {
        Ok(fd) => fd,
        Err(_) => std_process::exit(10),
    };

    let mut buffer = [0u8; PAYLOAD.len()];
    let data_result = io::read(connection, &mut buffer);
    let data_return_ms = role_now_or_exit(16);
    let data_offset_ms = data_return_ms.saturating_sub(epoch_ms);
    let data_latency_ms = data_offset_ms.saturating_sub(PEER_WRITE_AT_MS);
    let data_bytes = data_result.as_ref().copied().unwrap_or(0);
    println!(
        "LOOPBACK_WAKE_TEST: data offset_ms={} latency_ms={} bytes={}",
        data_offset_ms, data_latency_ms, data_bytes
    );
    let data_bytes = match data_result {
        Ok(bytes) => bytes,
        Err(_) => std_process::exit(11),
    };
    if data_bytes != PAYLOAD.len() || &buffer[..data_bytes] != PAYLOAD {
        std_process::exit(12);
    }
    let data_deadline_ms = epoch_ms
        .saturating_add(PEER_WRITE_AT_MS)
        .saturating_add(DATA_WAKE_BOUND_MS);
    if data_return_ms > data_deadline_ms {
        std_process::exit(13);
    }

    let eof_result = io::read(connection, &mut buffer);
    let eof_return_ms = role_now_or_exit(16);
    let eof_offset_ms = eof_return_ms.saturating_sub(epoch_ms);
    let eof_latency_ms = eof_offset_ms.saturating_sub(PEER_EXIT_AT_MS);
    let eof_bytes = eof_result.as_ref().copied().unwrap_or(0);
    println!(
        "LOOPBACK_WAKE_TEST: eof offset_ms={} latency_ms={} bytes={}",
        eof_offset_ms, eof_latency_ms, eof_bytes
    );
    if !matches!(eof_result, Ok(0)) {
        std_process::exit(14);
    }
    let eof_deadline_ms = epoch_ms
        .saturating_add(PEER_EXIT_AT_MS)
        .saturating_add(EOF_WAKE_BOUND_MS);
    if eof_return_ms > eof_deadline_ms {
        std_process::exit(15);
    }

    std_process::exit(0);
}

fn peer_child(epoch_ms: u64) -> ! {
    let connection = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => std_process::exit(20),
    };
    let server_addr = SockAddrIn::new([127, 0, 0, 1], LISTEN_PORT);
    if socket::connect_inet(connection, &server_addr).is_err() {
        std_process::exit(21);
    }

    sleep_until(epoch_ms, PEER_WRITE_AT_MS, 23);
    match io::write(connection, PAYLOAD) {
        Ok(bytes) if bytes == PAYLOAD.len() => {}
        _ => std_process::exit(22),
    }

    sleep_until(epoch_ms, PEER_EXIT_AT_MS, 23);
    // Do not close the socket: process teardown must emit the undrained FIN
    // whose delivery liveness is the subject of this regression test.
    std_process::exit(0);
}

fn load_child(epoch_ms: u64) -> ! {
    sleep_until(epoch_ms, LOAD_START_AT_MS, 30);

    // This child's only job is to deny idle_thread_fn the CPU across the FIN
    // delivery window, forcing kloopbackd rather than the idle drain to deliver.
    let end_ms = epoch_ms.saturating_add(LOAD_END_AT_MS);
    while role_now_or_exit(30) < end_ms {
        core::hint::spin_loop();
    }

    std_process::exit(0);
}

fn watchdog_child(epoch_ms: u64, reader_pid: Pid, peer_pid: Pid) -> ! {
    sleep_until(epoch_ms, WATCHDOG_AT_MS, 40);
    let _ = signal::kill(reader_pid.raw() as i32, 9);
    let _ = signal::kill(peer_pid.raw() as i32, 9);
    std_process::exit(0);
}

fn wait_status(pid: Pid) -> Option<i32> {
    let mut status = 0;
    match waitpid(pid.raw() as i32, &mut status, 0) {
        Ok(reaped) if reaped == pid => Some(status),
        _ => None,
    }
}

fn child_failure_reason(role: &str, status: Option<i32>) -> Option<String> {
    match status {
        Some(status) if wifexited(status) && wexitstatus(status) == 0 => None,
        Some(status) if wifexited(status) => {
            Some(format!("{}_exit_{}", role, wexitstatus(status)))
        }
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

    let epoch_ms = match monotonic_ms() {
        Some(now) => now,
        None => fail("clock"),
    };

    let reader_pid = match fork() {
        Ok(ForkResult::Child) => reader_child(server_fd, epoch_ms),
        Ok(ForkResult::Parent(pid)) => pid,
        Err(_) => fail("reader_fork"),
    };
    let peer_pid = match fork() {
        Ok(ForkResult::Child) => peer_child(epoch_ms),
        Ok(ForkResult::Parent(pid)) => pid,
        Err(_) => {
            let _ = signal::kill(reader_pid.raw() as i32, 9);
            fail("peer_fork");
        }
    };
    let load_pid = match fork() {
        Ok(ForkResult::Child) => load_child(epoch_ms),
        Ok(ForkResult::Parent(pid)) => pid,
        Err(_) => {
            let _ = signal::kill(reader_pid.raw() as i32, 9);
            let _ = signal::kill(peer_pid.raw() as i32, 9);
            fail("load_fork");
        }
    };
    let watchdog_pid = match fork() {
        Ok(ForkResult::Child) => watchdog_child(epoch_ms, reader_pid, peer_pid),
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
