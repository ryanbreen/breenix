#!/usr/bin/env python3
"""Run both virtio-blk wait implementations with a deterministic clock."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

from ahci_poller_race_test import function

ROOT = Path(__file__).resolve().parents[1]
HARNESS = """
#![allow(dead_code)]
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! { static NOW: Cell<u64> = const { Cell::new(0) }; }
fn monotonic_now_ns() -> u64 { NOW.get() }

mod task {
    pub mod completion {
        pub fn wait_sleeps() -> bool { true }
    }
}

struct Completion;
impl Completion {
    fn wait_timeout_uninterruptible(&self, _token: u32, ns: u64) -> Result<bool, ()> {
        NOW.set(NOW.get() + ns);
        // In this model the first used-ring notification arrives at 31 s.
        Ok(NOW.get() >= 31_000_000_000)
    }
}

struct BlockDeviceState;
fn device_has_failed(_: &BlockDeviceState) -> bool { false }
struct Transport;
impl Transport { fn has_failed(&self) -> bool { false } }
struct Waiter { completion: Completion, device: Transport }
"""


class VirtioBlockTimeout(unittest.TestCase):
    def test_runtime_request_deadline(self):
        for driver in ("block", "block_mmio"):
            with self.subTest(driver=driver), tempfile.TemporaryDirectory() as directory:
                source = (ROOT / f"kernel/src/drivers/virtio/{driver}.rs").read_text()
                wait = function(source, "wait_for_completion").replace("crate::task", "task")
                consts = re.findall(r"(?m)^const BLOCK_[A-Z_]+: u64 = [^;]+;", source)
                mmio = driver == "block_mmio"
                signature = "_: u32, _: u64" if mmio else "_: &Waiter, _: u32, _: u64"
                args = "1, &BlockDeviceState, 0" if mmio else "1, 0"
                harness = HARNESS + "\n".join(consts) + f"""
fn report_slow_request({signature}) {{}}
impl Waiter {{ {wait} }}

fn main() {{
    let waiter = Waiter {{ completion: Completion, device: Transport }};
    assert!(waiter.wait_for_completion({args}).is_err());
    assert_eq!(monotonic_now_ns(), 30_000_000_000);
}}
"""
                path = Path(directory)
                (path / "host.rs").write_text(harness)
                subprocess.run(["rustc", "--edition=2021", "-A", "unused_imports",
                                str(path / "host.rs"), "-o", str(path / "host")],
                               check=True, timeout=30)
                subprocess.run([str(path / "host")], check=True, timeout=10)


if __name__ == "__main__":
    unittest.main()
