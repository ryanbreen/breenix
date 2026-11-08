//! One non-GUI case observing the process state inherited from a scoped handoff.
// Like users_exec, enter from the C runtime directly: Rust startup changes
// SIGPIPE and reopens standard descriptors before an ordinary Rust main.
#![no_main]
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{io, process, signal, types::Fd};

fn reset_state() -> CaseResult {
    // Observe before prepare installs the next suite's watchdog.
    for number in 1..=64 {
        if number == signal::SIGKILL || number == signal::SIGSTOP { continue; }
        let mut action = signal::Sigaction::default();
        signal::sigaction(number, None, Some(&mut action))?;
        check(action.handler == signal::SIG_DFL, &format!("signal {number} retained its disposition"))?;
    }
    let mut mask = u64::MAX;
    signal::sigprocmask(signal::SIG_SETMASK, None, Some(&mut mask))?;
    check(mask == 0, "sequence retained a signal mask")?;
    for number in 3..4096 {
        check(io::fcntl(Fd::from_raw(number), io::fcntl_cmd::F_GETFD, 0).is_err(),
            &format!("sequence retained descriptor {number}"))?;
    }
    for fd in [Fd::STDOUT, Fd::STDERR] {
        check(io::fcntl(fd, io::fcntl_cmd::F_GETFD, 0).is_ok(), "sequence closed a standard output descriptor")?;
    }
    let pid = process::getpid()?.raw();
    let runner: u64 = std::env::var("BREENIX_UI_SEQUENCE_RUNNER").map_err(|e| e.to_string())?
        .parse().map_err(|_| "invalid runner pid")?;
    check(pid == runner && pid > 1, "sentinel did not replace the scoped runner")?;
    let mut parents = Vec::new();
    for entry in std::fs::read_dir("/proc").map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(id) = entry.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else { continue; };
        let stat = std::fs::read_to_string(entry.path().join("stat")).map_err(|e| e.to_string())?;
        let parent = stat.rsplit_once(") ").and_then(|(_, s)| s.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u64>().ok()).ok_or("invalid parent table")?;
        parents.push((id, parent));
    }
    check(breenix_suite::suite::descendants::descendant_victims(pid, &parents).is_empty(),
        "fixture descendants survived into sentinel")?;
    let mut status = 0;
    check(matches!(process::waitpid(-1, &mut status, process::WNOHANG),
        Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ECHILD))), "fixture retained a child or zombie")
}

#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8, _envp: *const *const u8) -> i32 {
    let result = reset_state();
    let place = breenix_suite::suite::sequence::read("sequence-sentinel").expect("sentinel sequence membership");
    breenix_suite::suite::sequence::prepare(&place.0, place.1, 1);
    println!("SUITE sequence-sentinel START cases=1");
    match &result {
        Ok(()) => println!("SUITE sequence-sentinel CASE reset/state PASS ms=0"),
        Err(error) => println!("SUITE sequence-sentinel CASE reset/state FAIL ms=0 msg={error:?}"),
    }
    println!("SUITE sequence-sentinel DONE passed={} failed={} skipped=0 total=1",
        usize::from(result.is_ok()), usize::from(result.is_err()));
    breenix_suite::suite::sequence::finish();
    i32::from(result.is_err())
}
