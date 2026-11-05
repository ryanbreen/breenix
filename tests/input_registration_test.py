"""Force same-CPU preemption at production registration lock acquisition."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from input_retirement_test import item

ROOT = Path(__file__).resolve().parents[1]

class Registration(unittest.TestCase):
    def test_worker_registration_excludes_syscall_preemption(self):
        register = item((ROOT / "kernel/src/input/stream.rs").read_text(), "pub fn register_hardware(")
        replay = item((ROOT / "kernel/src/input/control.rs").read_text(), "pub fn register(")
        program = r"""
#![allow(dead_code)]
use std::{cell::Cell, collections::BTreeMap, sync::{Mutex, MutexGuard}};
thread_local! { static MASKED: Cell<bool> = const { Cell::new(false) }; }
fn arch_without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let was = MASKED.replace(true);
    let result = f();
    MASKED.set(was);
    result
}
struct Lock<T>(Mutex<T>);
impl<T> Lock<T> {
    const fn new(value: T) -> Self { Self(Mutex::new(value)) }
    fn lock(&self) -> MutexGuard<'_, T> {
        let guard = self.0.lock().unwrap();
        // Force the timer to schedule a syscall immediately after worker acquisition.
        if !MASKED.get() {
            assert!(self.0.try_lock().is_ok(), "same-CPU syscall spins on preempted registration owner");
        }
        guard
    }
}
mod input {
    pub mod hardware {
        pub const DEVICE_LIMIT: usize = 1024;
        pub struct Description { pub capabilities: Vec<()>, pub keyboard: Option<u32> }
        pub struct Device { pub id: u32, pub keyboard: Option<u32> }
    }
    pub mod stream {
        use super::super::*;
        struct Device { capabilities: Vec<()>, injector_time: bool, last_time: u64,
            modifiers: u8, reserved: u16, held: BTreeMap<u16, ()> }
        struct State { next_hardware: u32, devices: BTreeMap<u32, Device> }
        static STATE: Lock<State> = Lock::new(State { next_hardware: 1, devices: BTreeMap::new() });
""" + register + r"""
    }
    pub mod control {
        use super::super::*;
        pub trait Replay: Sync {}
        static BOUND: Lock<[Option<&'static dyn Replay>; 2]> = Lock::new([None, None]);
""" + replay + r"""
    }
}
struct Driver;
impl input::control::Replay for Driver {}
static DRIVER: Driver = Driver;
fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("replay") => input::control::register(&DRIVER, false),
        _ => { input::stream::register_hardware(input::hardware::Description {
            capabilities: Vec::new(), keyboard: Some(1) }).unwrap(); }
    }
    assert!(!MASKED.get(), "caller interrupt state was not restored");
}
"""
        with tempfile.TemporaryDirectory(prefix="input-registration-") as temp:
            source = Path(temp) / "check.rs"
            binary = Path(temp) / "check"
            source.write_text(program)
            command = (["taskpolicy", "-b"] if sys.platform == "darwin" else [])
            result = subprocess.run(command + ["rustc", "--edition=2021", "-Dwarnings", str(source),
                "-o", str(binary)], capture_output=True, text=True,
                env={**os.environ, "CARGO_BUILD_JOBS": "4"})
            self.assertEqual(result.returncode, 0, result.stderr)
            for case in ["hardware", "replay"]:
                with self.subTest(case=case):
                    result = subprocess.run([str(binary), case], capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stderr)

if __name__ == "__main__":
    unittest.main()
