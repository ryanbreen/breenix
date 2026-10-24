#!/usr/bin/env python3
"""Execute production admission bodies with a deterministic preempting lock.

The host model attempts a same-CPU, nonpreemptible syscall immediately after
acquisition. A busy lock here means the suspended worker cannot run to release
it. This is a bounded deadlock detector, not hardware incident attribution.
Pass --revision COMMIT to run the identical check on a historical driver.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def function(source, signature):
    start = source.index(signature)
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


MODEL = r'''
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static IRQ_ENABLED: AtomicBool = AtomicBool::new(true);
static STRANDED: AtomicUsize = AtomicUsize::new(0);
static TICKS: AtomicUsize = AtomicUsize::new(0);
fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let old = IRQ_ENABLED.swap(false, Ordering::SeqCst);
    let result = f();
    IRQ_ENABLED.store(old, Ordering::SeqCst);
    result
}
struct PreemptMutex<T>(Mutex<T>);
impl<T> PreemptMutex<T> {
    const fn new(value: T) -> Self { Self(Mutex::new(value)) }
    fn lock(&self) -> MutexGuard<'_, T> {
        let guard = self.0.lock().unwrap();
        // Deliver a tick at precisely the lock-held instruction boundary.
        TICKS.fetch_add(1, Ordering::SeqCst);
        if IRQ_ENABLED.load(Ordering::SeqCst) {
            arch_without_interrupts(|| {
                // The syscall cannot finish or deliver pending KILL until
                // this acquisition completes; its worker cannot resume.
                if self.0.try_lock().is_err() {
                    STRANDED.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
        guard
    }
}
mod random {
    pub enum Event { TtyInput }
    pub fn add_event(_: Event, _: u64) {}
}
mod tty {
    #[derive(Clone, Copy)]
    pub struct InputSource;
    pub mod input_clock { pub fn record_input() {} }
    pub mod driver {
        use super::super::*;
        pub struct Discipline;
        impl Discipline {
            fn termios(&self) -> &u8 { &0 }
            fn input_char(&mut self, c: u8, echo: &mut dyn FnMut(u8)) -> Option<u32> { echo(c); None }
            fn flow_stopped(&self) -> bool { false }
        }
        struct Sources;
        impl Sources { fn record(&self, _: super::InputSource) {} }
        pub struct TtyDevice {
            num: u8, ldisc: PreemptMutex<Discipline>, input_sources: Sources,
            input_flow_stopped: AtomicBool,
        }
        impl TtyDevice {
            pub fn new() -> Self {
                Self { num: 0, ldisc: PreemptMutex::new(Discipline), input_sources: Sources,
                       input_flow_stopped: AtomicBool::new(false) }
            }
            fn echo_char(&self, _: u8, _: &u8) {}
            fn publish_serial_flow(&self) {}
            fn transfer_input(&self, _: &mut Discipline) -> bool { true }
            fn send_signal_to_foreground(&self, _: u32) {}
            fn wake_blocked_readers() {}
            INPUT_METHOD
        }
        static CONSOLE_TTY: PreemptMutex<Option<Arc<TtyDevice>>> = PreemptMutex::new(None);
        CONSOLE_FUNCTION
    }
}
#[test]
fn preempt_in_line_discipline_cannot_strand_a_syscall() {
    STRANDED.store(0, Ordering::SeqCst);
    TICKS.store(0, Ordering::SeqCst);
    tty::driver::TtyDevice::new().input_char_from(b'x', tty::InputSource);
    assert!(TICKS.load(Ordering::SeqCst) > 0, "no acquisition exercised");
    assert_eq!(STRANDED.load(Ordering::SeqCst), 0, "syscall stranded behind preempted ldisc holder");
}
#[test]
fn preempt_in_console_lookup_cannot_strand_a_syscall() {
    STRANDED.store(0, Ordering::SeqCst);
    TICKS.store(0, Ordering::SeqCst);
    assert!(tty::driver::console().is_none());
    assert!(TICKS.load(Ordering::SeqCst) > 0, "no acquisition exercised");
    assert_eq!(STRANDED.load(Ordering::SeqCst), 0, "syscall stranded behind preempted console holder");
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--revision')
    args = parser.parse_args()
    path = 'kernel/src/tty/driver.rs'
    source = subprocess.check_output(['git', 'show', args.revision + ':' + path], cwd=ROOT, text=True) if args.revision else (ROOT / path).read_text()
    model = MODEL.replace('INPUT_METHOD', function(source, 'pub fn input_char_from(')).replace('CONSOLE_FUNCTION', function(source, 'pub fn console()'))
    with tempfile.TemporaryDirectory(prefix='tty-preempt-') as scratch:
        rust = Path(scratch) / 'model.rs'
        rust.write_text(model)
        subprocess.run(['rustc', '--edition=2021', '-Dwarnings', '--test', str(rust), '-o', str(rust.with_suffix(''))], cwd=ROOT, check=True)
        return subprocess.run([str(rust.with_suffix('')), '--test-threads=1'], cwd=ROOT).returncode


if __name__ == '__main__':
    raise SystemExit(main())
