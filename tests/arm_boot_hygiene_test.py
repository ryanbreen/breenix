#!/usr/bin/env python3
"""Run the boot-hygiene checks against the production service-table parser."""

import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
RUST_TEST = r'''
//! Default-boot daemon parity and the unauthenticated telnet door.

#[cfg(test)]
mod tests {
use libbreenix::services::{check_needs, parse_into, Service};
use std::collections::BTreeSet;

fn table(arch: &str, text: &str) -> Vec<Service> {
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    parse_into(text, arch, &mut entries, &mut problems);
    check_needs(&mut entries, &mut problems);
    assert!(problems.is_empty(), "{arch}: {problems:?}");
    entries
}

fn tables() -> (Vec<Service>, Vec<Service>) {
    (
        table("aarch64", include_str!(concat!(env!("BREENIX_ROOT"), "/libs/libbreenix/src/service-tables/aarch64"))),
        table("x86_64", include_str!(concat!(env!("BREENIX_ROOT"), "/libs/libbreenix/src/service-tables/x86_64"))),
    )
}

#[test]
fn daemon_sets_match() {
    let (arm, x86) = tables();
    let daemons = |entries: &[Service]| -> BTreeSet<String> {
        entries.iter().filter(|s| !s.name.starts_with("getty-")).map(|s| s.name.clone()).collect()
    };
    assert_eq!(daemons(&arm), daemons(&x86));
}

#[test]
fn logging_and_time_daemons_match_x86_policy() {
    let (arm, x86) = tables();
    for name in ["blogd", "ntp"] {
        let find = |entries: &[Service]| entries.iter().find(|s| s.name == name).cloned();
        let expected = find(&x86).unwrap_or_else(|| panic!("x86_64 lacks {name}"));
        let actual = find(&arm).unwrap_or_else(|| panic!("aarch64 lacks {name}"));
        assert_eq!(actual.needs, expected.needs, "{name} needs");
        assert_eq!(actual.restart, expected.restart, "{name} restart");
    }
}

#[test]
fn init_does_not_start_telnetd() {
    let init = include_str!(concat!(env!("BREENIX_ROOT"), "/userspace/programs/src/init.rs"));
    assert!(!init.contains("telnetd"), "init must not spawn telnetd on any architecture");
}
}
'''


CONSOLE_TEST = r"""
#[cfg(test)]
mod console_mode {
    use std::sync::atomic::{AtomicU64, Ordering};
    static PID: AtomicU64 = AtomicU64::new(1);
    struct Pid(u64);
    impl Pid { fn raw(self) -> u64 { self.0 } }
    fn getpid() -> Result<Pid, ()> { Ok(Pid(PID.load(Ordering::Relaxed))) }
    fn detach_console() {}
    #[cfg(target_arch = "x86_64")]
    fn console_text_mode() {}
    mod init_services {
        #[derive(Debug, PartialEq, Eq, Clone, Copy)]
        pub enum Scope { All, Logins }
        pub struct Supervisor(Scope, bool);
        impl Supervisor {
            pub fn load(scope: Scope, publish: bool) -> Self { Self(scope, publish) }
            pub fn run(self) -> ! { std::panic::panic_any((self.0, self.1)) }
        }
    }
    __CONSOLE_FUNCTION__
    #[test]
    fn production_console_starts_daemons_without_changing_child_login_fixtures() {
        for (pid, scope) in [(1, init_services::Scope::All), (42, init_services::Scope::Logins)] {
            PID.store(pid, Ordering::Relaxed);
            let stopped = std::panic::catch_unwind(run_console_mode).expect_err("supervisor runs forever");
            let actual = *stopped.downcast::<(init_services::Scope, bool)>().expect("supervisor selection");
            assert_eq!(actual, (scope, pid == 1), "console init PID {pid}");
        }
    }
}
"""


def console_function():
    source = (ROOT / "userspace/programs/src/init.rs").read_text()
    start = source.index("fn run_console_mode()")
    opening = source.index("{", start)
    depth = 0
    for at in range(opening, len(source)):
        depth += (source[at] == "{") - (source[at] == "}")
        if depth == 0:
            return source[start:at + 1]
    raise ValueError("unterminated run_console_mode")

if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="breenix-boot-hygiene-") as directory:
        crate = Path(directory)
        (crate / "src").mkdir()
        (crate / "src/lib.rs").write_text(RUST_TEST + CONSOLE_TEST.replace("__CONSOLE_FUNCTION__", console_function()))
        library = ROOT / "libs/libbreenix"
        (crate / "Cargo.toml").write_text(
            '[package]\nname = "boot-hygiene-check"\nversion = "0.1.0"\nedition = "2021"\n'
            f'[dependencies]\nlibbreenix = {{ path = "{library}", features = ["std"] }}\n'
        )
        env = os.environ.copy()
        env["BREENIX_ROOT"] = str(ROOT)
        raise SystemExit(subprocess.call(["cargo", "test", "--release", "--manifest-path", str(crate / "Cargo.toml")], env=env))
