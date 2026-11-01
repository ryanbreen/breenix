#!/usr/bin/env python3
"""Exercise the production SNTP polling loop with a virtual clock and sources."""

from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]

# Compile the actual polling loop, replacing only its clock, source discovery
# and network exchange. No privileged clock or filesystem syscall runs here.
source = (ROOT / "userspace/programs/src/ntp_client.rs").read_text()
polling = source[source.index("    let mut retry = 16;"):source.index("\nfn main()")]
harness = r'''
use std::cell::{Cell, RefCell};
const POLL: u64 = 3600;
thread_local! {
    static NOW: Cell<u64> = const { Cell::new(0) };
    static DISCOVERY: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}
fn mono() -> u64 { NOW.with(Cell::get) }
fn sleep(ms: u64) { NOW.with(|t| t.set(t.get() + ms * 1_000_000)); }
struct State { syncs: u64, last: u64 }
impl State { fn load() -> Self { Self { syncs: 0, last: 0 } } }
fn sources() -> Result<(Vec<String>, &'static str), String> {
    DISCOVERY.with(|calls| {
        let mut calls = calls.borrow_mut();
        calls.push(mono());
        if calls.len() < 3 { Err("no site or DHCP SNTP servers".into()) }
        else { Ok((vec!["time.example.invalid".into()], "site")) }
    })
}
fn synchronize(name: &str, port: u16, source: &str) -> Result<(), String> {
    assert_eq!((name, port, source), ("time.example.invalid", 123, "site"));
    panic!("reached newly configured peer");
}
fn poll(once: bool) -> Result<(), String> {
    let server: Option<String> = None;
    let port = 123;
''' + polling + r'''
#[test]
fn daemon_retries_discovery_then_uses_new_configuration() {
    let result = std::panic::catch_unwind(|| poll(false));
    assert!(result.is_err(), "daemon exited instead of discovering the new peer: {result:?}");
    DISCOVERY.with(|calls| assert_eq!(*calls.borrow(), [0, 16_000_000_000, 48_000_000_000]));
}
#[test]
fn once_reports_missing_sources_without_retrying() {
    assert_eq!(poll(true), Err("no site or DHCP SNTP servers".into()));
    DISCOVERY.with(|calls| assert_eq!(*calls.borrow(), [0]));
}
'''

if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="breenix-ntp-retry-") as directory:
        path = Path(directory)
        (path / "check.rs").write_text(harness)
        subprocess.run(["rustc", "--edition=2021", "--test", "-Dwarnings",
                        str(path / "check.rs"), "-o", str(path / "check")], check=True)
        raise SystemExit(subprocess.call([str(path / "check")]))
