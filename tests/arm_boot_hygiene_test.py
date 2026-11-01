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

if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="breenix-boot-hygiene-") as directory:
        crate = Path(directory)
        (crate / "src").mkdir()
        (crate / "src/lib.rs").write_text(RUST_TEST)
        library = ROOT / "libs/libbreenix"
        (crate / "Cargo.toml").write_text(
            '[package]\nname = "boot-hygiene-check"\nversion = "0.1.0"\nedition = "2021"\n'
            f'[dependencies]\nlibbreenix = {{ path = "{library}", features = ["std"] }}\n'
        )
        env = os.environ.copy()
        env["BREENIX_ROOT"] = str(ROOT)
        raise SystemExit(subprocess.call(["cargo", "test", "--release", "--manifest-path", str(crate / "Cargo.toml")], env=env))
