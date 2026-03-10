//! HTTP fetch test for bcheck.
//!
//! Performs an HTTP GET to http://www.duke.edu/ and verifies we get
//! a valid response (2xx or 3xx redirect). Tests the full network stack:
//! DNS resolution, TCP connect, HTTP request/response parsing.
//!
//! Exits 0 on success, 1 on failure.

use libbreenix::http::{self, HttpError, MAX_RESPONSE_SIZE};
use libbreenix::process::{fork, ForkResult};
use libbreenix::time::sleep_ms;
use std::process;

fn main() {
    println!("[http_fetch_test] Fetching http://www.duke.edu/");

    // Fork a child to do the HTTP request with a timeout.
    // The parent enforces a 15-second deadline so bcheck doesn't hang.
    match fork() {
        Ok(ForkResult::Child) => {
            do_http_fetch();
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Wait up to 15 seconds for the child
            let mut status: i32 = 0;
            let start = libbreenix::time::now_monotonic()
                .unwrap_or(libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 0 });
            let deadline = start.tv_sec as u64 + 15;

            loop {
                match libbreenix::process::waitpid(
                    child_pid.raw() as i32,
                    &mut status as *mut i32,
                    libbreenix::process::WNOHANG,
                ) {
                    Ok(pid) if pid.raw() > 0 => {
                        // Child exited — check wait status per POSIX
                        let signaled = (status & 0x7F) != 0;
                        if signaled {
                            let sig = status & 0x7F;
                            println!("[http_fetch_test] Child killed by signal {}", sig);
                            process::exit(1);
                        }
                        let exit_code = (status >> 8) & 0xFF;
                        if exit_code == 0 {
                            process::exit(0);
                        } else {
                            println!("[http_fetch_test] Child exited with code {}", exit_code);
                            process::exit(1);
                        }
                    }
                    _ => {}
                }

                let now = libbreenix::time::now_monotonic()
                    .unwrap_or(libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 0 });
                if now.tv_sec as u64 >= deadline {
                    println!("[http_fetch_test] FAIL (timeout after 15s)");
                    // Kill child
                    let _ = libbreenix::signal::kill(child_pid.raw() as i32, 9);
                    let _ = libbreenix::process::waitpid(child_pid.raw() as i32, &mut status, 0);
                    process::exit(1);
                }

                let _ = sleep_ms(50);
            }
        }
        Err(_) => {
            println!("[http_fetch_test] FAIL (fork failed)");
            process::exit(1);
        }
    }
}

fn do_http_fetch() -> ! {
    // Heap-allocate: MAX_RESPONSE_SIZE (64KB) would overflow the 64KB user stack
    let mut buf = vec![0u8; MAX_RESPONSE_SIZE];
    match http::http_get_with_buf("http://www.duke.edu/", &mut buf) {
        Ok((response, _total_len)) => {
            if response.status_code >= 200 && response.status_code < 400 {
                println!(
                    "[http_fetch_test] PASS (status {} body={}B)",
                    response.status_code, response.body_len
                );
                libbreenix::process::exit(0);
            } else {
                println!(
                    "[http_fetch_test] FAIL (unexpected status {})",
                    response.status_code
                );
                libbreenix::process::exit(1);
            }
        }
        Err(HttpError::Timeout) => {
            println!("[http_fetch_test] FAIL (timeout)");
            libbreenix::process::exit(1);
        }
        Err(HttpError::DnsError(e)) => {
            println!("[http_fetch_test] FAIL (DNS: {:?})", e);
            libbreenix::process::exit(1);
        }
        Err(e) => {
            println!("[http_fetch_test] FAIL ({:?})", e);
            libbreenix::process::exit(1);
        }
    }
}
