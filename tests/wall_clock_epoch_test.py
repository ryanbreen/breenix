"""Drive the x86 coarse wall clock with a scheduler timer started after calibration."""

import subprocess
import tempfile
import unittest
from pathlib import Path

TIME = Path(__file__).resolve().parents[1] / "kernel/src/time"


def function(source, name):
    start = source.index(f"fn {name}(")
    opening = source.index("{", start)
    depth = 1
    for end in range(opening + 1, len(source)):
        if source[end] == "{":
            depth += 1
        elif source[end] == "}":
            depth -= 1
            if depth == 0:
                return source[start : end + 1]
    raise AssertionError(f"unterminated Rust function {name}")


HARNESS = r"""
#![allow(dead_code)]
extern crate self as log;
extern crate self as x86_64;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { let _ = format_args!($($arg)*); };
}

mod instructions {
    pub mod port {
        pub struct Port<T>(std::marker::PhantomData<T>);
        impl Port<u8> {
            pub const fn new(_: u16) -> Self { Self(std::marker::PhantomData) }
            pub unsafe fn read(&mut self) -> u8 { unreachable!() }
            pub unsafe fn write(&mut self, _: u8) { unreachable!() }
        }
    }
}

mod arch_impl {
    pub mod x86_64 {
        pub mod apic {
            pub fn active() -> bool { true }
            pub fn calibrate_timer(_: u32) -> u32 { 1 }
            pub fn timer_state() -> (u32, u32, u32) { unreachable!() }
        }
        pub mod timer {
            pub fn nanoseconds_at_tsc(sample: u64) -> Option<u64> {
                Some(sample - crate::tsc::CALIBRATION_EDGE)
            }
        }
    }
}

mod interrupts {
    pub fn validate_pic_irq0_unmasked() -> (bool, bool) { unreachable!() }
}

mod rtc {
    pub fn init() {}
    pub fn get_boot_wall_time() -> u64 { 1_795_159_039 }
}

mod tsc {
    use super::*;
    pub const CALIBRATION_EDGE: u64 = 50_000;
    pub static NOW: AtomicU64 = AtomicU64::new(CALIBRATION_EDGE);
    pub static HARDWARE_READS: AtomicBool = AtomicBool::new(true);
    pub fn frequency_hz() -> u64 { 1_000_000_000 }
    pub fn read_tsc() -> u64 {
        assert!(HARDWARE_READS.load(Ordering::Relaxed), "coarse clock read the counter");
        NOW.load(Ordering::Relaxed)
    }
    pub fn read_tsc_serialized() -> u64 { read_tsc() }
    pub fn monotonic_time() -> Option<(u64, u64)> {
        let elapsed = read_tsc() - CALIBRATION_EDGE;
        Some((elapsed / frequency_hz(), elapsed % frequency_hz()))
    }
}

mod timer;
mod slew;
use timer::{get_monotonic_time, get_monotonic_time_ns};

fn main() {
    // The R240 census paired tick*5 with high-resolution milliseconds at a
    // separation of 1,407 ms near metadata/atime-advances (E24 ledger 710).
    tsc::NOW.fetch_add(1_407_000_000, Ordering::Relaxed);
    timer::init();
    let created = current_unix_time();
    tsc::NOW.fetch_add(1_100_000_000, Ordering::Relaxed);
    timer::timer_interrupt();
    assert_eq!(get_monotonic_time(), 1_100, "scheduler timer keeps its own epoch");
    let now = current_unix_time();
    tsc::HARDWARE_READS.store(false, Ordering::Relaxed);
    let read = current_unix_time_coarse();
    assert!(read > created, "read stamp {read} did not advance creation stamp {created}");
    assert_eq!(read, now, "coarse and precise wall time must share the calibration epoch");
}
"""


class WallClockEpochTest(unittest.TestCase):
    def test_coarse_wall_time_advances_across_a_read(self):
        wall = (TIME / "mod.rs").read_text()
        state = wall[wall.index("static SLEW_START:") : wall.index("pub fn set_real_time_ns(")]
        readers = [function(wall, name) for name in
                   ("get_real_time_ns", "current_unix_time", "current_unix_time_coarse")]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # Select the production x86 functions even on an ARM host.
            timer = (TIME / "timer.rs").read_text().replace('target_arch = "x86_64"', "all()")
            (root / "timer.rs").write_text(timer.replace('target_arch = "aarch64"', "any()"))
            (root / "slew.rs").write_text((TIME / "slew.rs").read_text())
            (root / "main.rs").write_text("\n".join([HARNESS, state, *readers]))
            subprocess.run(["rustc", "--edition=2021", "-Dwarnings", "main.rs", "-o", "check"],
                           cwd=root, check=True, timeout=30)
            result = subprocess.run([str(root / "check")], capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
