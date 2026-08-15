//! Event-driven userspace regression test for issue #545, where a reader
//! blocked in TCP `recv()` on an x86 loopback connection could remain asleep
//! forever.
//!
//! Epoch-relative scheduling was tried first, but produced a false failure on
//! a healthy x86 kernel: during the busy userspace boot phase, four `fork()`s
//! plus `accept`/`connect`/handshake setup took about four seconds, so every
//! scheduled deadline had expired before its child first ran. This version
//! instead makes every ordering constraint an observed pipe event and measures
//! data delivery from a timestamp taken by the peer immediately before its
//! write. Pipe synchronization cannot drain the loopback queue.
//!
//! The peer exits without closing its socket, so process teardown emits the
//! undrained FIN under test. A load child keeps the CPU busy across that window
//! so `kloopbackd`, rather than the idle-loop drain, must deliver it. A watchdog
//! is the sole bounded escape if a blocking read never wakes.

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::process::{fork, waitpid, wexitstatus, wifexited, ForkResult};
use libbreenix::signal;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::time::{now_monotonic, sleep_ms};
use libbreenix::types::{Fd, Pid};
use std::process as std_process;

const LISTEN_PORT: u16 = 54530;
const TAG: &[u8] = b"545-wake";
const PAYLOAD_LEN: usize = 16;
const DATA_WAKE_BOUND_MS: u64 = 4000;
const EOF_WAKE_BOUND_MS: u64 = 4000;
// A healthy kernel returned EOF in 1500 ms, against this 4000 ms bound. If
// loopback delivery liveness is lost, the FIN cannot arrive until this 12000 ms
// spin ends and lets the idle drain run.
const LOAD_SPIN_MS: u64 = 12000;
const WATCHDOG_AT_MS: u64 = 30000;

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

fn load_child(ready_r: Fd, spin_w: Fd) -> ! {
    let mut ready_signal = [0u8; 1];
    match io::read(ready_r, &mut ready_signal) {
        Ok(0) => std_process::exit(31),
        Ok(_) => {}
        Err(_) => std_process::exit(32),
    }

    let start_ms = role_now_or_exit(30);
    match io::write(spin_w, b"s") {
        Ok(1) => {}
        _ => std_process::exit(33),
    }

    // Never block after releasing the peer: denying idle_thread_fn the CPU
    // across the FIN window makes kloopbackd, not the idle drain, deliver it.
    let end_ms = start_ms.saturating_add(LOAD_SPIN_MS);
    while role_now_or_exit(30) < end_ms {
        core::hint::spin_loop();
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
