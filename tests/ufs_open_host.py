#!/usr/bin/env python3
"""Exercise the production open prologue while an unrelated write holds its gate."""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/fs/ufs/file.rs").read_text()
start = source.index("        let lease = super::open_file(mount)?;")
end = source.index("        let creating =", start)
prologue = source[start:end]
if "let mutation =" in prologue:
    prologue += "        drop(mutation);\n"
harness = r'''
use std::sync::atomic::{AtomicBool, Ordering};
static WRITE_PENDING: AtomicBool = AtomicBool::new(true);
struct Gate;
impl Gate {
    fn enter(&self) -> Result<Guard, u64> {
        assert!(!WRITE_PENDING.load(Ordering::Relaxed),
            "an unrelated pending write blocks an ordinary open");
        Ok(Guard)
    }
}
struct Guard;
impl Drop for Guard { fn drop(&mut self) {} }
struct Mount { mutations: Gate }
struct Lease(Mount);
fn open_file(_: u64) -> Result<Lease, u64> { Ok(Lease(Mount { mutations: Gate })) }
mod production {
    pub fn begin_open(mount: u64) -> Result<(), u64> {
        PROLOGUE
        let _ = &lease.0.mutations;
        let _enter = super::Gate::enter;
        Ok(())
    }
}
#[test]
fn ordinary_open_proceeds_while_write_pending() {
    production::begin_open(1).unwrap();
}
'''.replace("PROLOGUE", prologue)
with tempfile.TemporaryDirectory(prefix="breenix-ufs-open-") as temporary:
    path = Path(temporary)
    test = path / "check.rs"
    test.write_text(harness)
    subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "-Dwarnings",
                    "--test", str(test), "-o", str(path / "check")], check=True)
    subprocess.run([str(path / "check")], check=True)
