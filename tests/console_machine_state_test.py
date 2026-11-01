#!/usr/bin/env python3
"""Execute console startup with a stand-in supervisor to observe state import."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ConsoleMachineState(unittest.TestCase):
    def test_pid_one_imports_before_services_and_child_does_not(self):
        source = (ROOT / 'userspace/programs/src/init.rs').read_text()
        start = source.index('fn run_console_mode()')
        end = source.index('\n}\n', start) + 2
        function = source[start:end]
        harness = r'''
use std::cell::Cell;
thread_local! {
    static PID: Cell<u32> = const { Cell::new(1) };
    static IMPORTS: Cell<u32> = const { Cell::new(0) };
    static BEFORE_SERVICES: Cell<u32> = const { Cell::new(0) };
}
struct Pid(u32);
impl Pid { fn raw(&self) -> u32 { self.0 } }
fn getpid() -> Result<Pid, ()> { Ok(Pid(PID.with(Cell::get))) }
fn console_text_mode() {}
fn detach_console() {}
fn reconcile_machine_state() { IMPORTS.with(|c| c.set(c.get() + 1)); }
mod init_services {
    pub enum Scope { All, Logins }
    pub struct Supervisor;
    impl Supervisor {
        pub fn load(_: Scope, _: bool) -> Self {
            super::BEFORE_SERVICES.with(|c| c.set(super::IMPORTS.with(std::cell::Cell::get)));
            Self
        }
        pub fn run(self) -> ! { panic!("supervisor reached") }
    }
}
'''
        main = r'''
fn main() {
    let _ = (console_text_mode as fn(), reconcile_machine_state as fn());
    for (pid, expected) in [(1, 1), (2, 0)] {
        PID.with(|c| c.set(pid));
        IMPORTS.with(|c| c.set(0));
        BEFORE_SERVICES.with(|c| c.set(0));
        let _ = std::panic::catch_unwind(run_console_mode);
        assert_eq!(IMPORTS.with(Cell::get), expected, "PID {pid} state import count");
        assert_eq!(BEFORE_SERVICES.with(Cell::get), expected, "PID {pid} services started before import");
    }
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness + function + main)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
