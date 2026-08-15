//! Exec smoke target — the image /bin/exec_smoke execs into.
//!
//! Sleeps and yields BEFORE printing its marker so the post-exec thread is descheduled and
//! redispatched at least once. That round trip is what the exec receipt (ExecSchedCommit) exists to
//! make correct: the scheduler-side copy of the thread context is written after the process manager
//! lock is released, and a stale or missing copy is the historical `elr_el1 = 0` crash.

use libbreenix::process::yield_now;
use libbreenix::types::Timespec;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("[EXEC_SMOKE:TARGET_ENTER argc={}]", args.len());

    let argv_ok = args.len() == 2 && args[0] == "exec_smoke_target" && args[1] == "smoke";
    if !argv_ok {
        println!(
            "[EXEC_SMOKE:TARGET_ARGV_FAIL argc={} args={:?}]",
            args.len(),
            args
        );
        std::process::exit(1);
    }

    let ts = Timespec {
        tv_sec: 0,
        tv_nsec: 100_000_000,
    };
    let _ = libbreenix::time::nanosleep(&ts);
    for _ in 0..8 {
        let _ = yield_now();
    }

    println!("[EXEC_SMOKE:TARGET_OK]");
    std::process::exit(0);
}
