"""Run device-repeat suppression through production disposition and ledger code.

The descriptor/ioctl boundary is in process; no guest execution is claimed.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class RepeatLedger(unittest.TestCase):
    def test_suppressed_repeat_has_a_terminal_source_row(self):
        stream = (ROOT / 'kernel/src/input/stream.rs').read_text()
        bwm_input = (ROOT / 'userspace/programs/src/bwm/input.rs').read_text()
        main = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        # Keep the real early-continue path, including its finish call.
        suppress = main[main.index('            if event.kind == kind::KEY && event.value == 2'):
                        main.index('            let current_mods =')]
        record = item(stream, '            ioctl::DISPOSITION =>')
        record = record[record.index('{') + 1:-1]
        program = r'''
extern crate alloc;
extern crate self as libbreenix;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use input_abi::{disposition, reason, kind, Event, Disposition};
pub mod input { pub use input_abi::*; }
pub mod ui_clock { pub fn now_ns() -> u64 { 100 } }
pub mod errno { #[derive(Debug)] pub enum Errno { EALREADY, Other } }
pub mod error {
    #[derive(Debug)] pub enum Error { Os(super::errno::Errno) }
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
    }
}
struct Lock<T>(Mutex<T>);
impl<T> Lock<T> { fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() } }
struct Meta { rejected: [u64; 8], rejected_count: u32 }
struct Window { meta: Meta }
struct Registry { windows: BTreeMap<u64, Window> }
mod window { use super::*; static REG: Lock<Registry> = Lock(Mutex::new(Registry { windows: BTreeMap::new() }));
    pub(super) use self::REG as REGISTRY;
}
'''.replace('static REG:', 'pub(super) static REG:')
        for name in ['Row', 'Grab', 'Held']:
            program += item(stream, f'struct {name} {{') + '\n'
        program += r'''
struct State { ledger: VecDeque<Row>, grab: Option<Grab>,
    derivations: BTreeMap<(u64, u32), Disposition>, recipients: BTreeMap<(u32, u64), Held> }
static STATE: Lock<State> = Lock(Mutex::new(State { ledger: VecDeque::new(), grab: None,
    derivations: BTreeMap::new(), recipients: BTreeMap::new() }));
const EINVAL: u32 = 22; const ENOENT: u32 = 2; const EPERM: u32 = 1; const EALREADY: u32 = 114;
struct Caller { tgid: u64 }
fn wake_service() {}
fn record(d: Disposition) -> Result<u64, u64> {
    let outcome = Some(d); let reader = 7; let caller = Caller { tgid: 1 }; let epoch = 1;
    let reg = window::REGISTRY.lock();
'''
        program += record + '\n}\n'
        program += item(stream, 'fn row_mut(') + '\n'
        program += item(stream, 'pub fn ledger(') + '\n'
        program += f'#[path = "{ROOT / "userspace/programs/src/bwm/timing.rs"}"] mod timing;\n'
        program += r'''
mod bwm_input {
    use super::*;
    use crate::input as core_input;
    use input_abi::{disposition as outcome, reason};
    struct Reader;
    impl Reader {
        fn dispose(&self, d: &Disposition) -> Result<(), error::Error> {
            record(*d).map(|_| ()).map_err(|e| error::Error::Os(if e == EALREADY as u64 {
                errno::Errno::EALREADY } else { errno::Errno::Other }))
        }
    }
    static READER: std::sync::OnceLock<Reader> = std::sync::OnceLock::new();
    pub fn initialize() { assert!(READER.set(Reader).is_ok()); }
'''
        program += bwm_input[bwm_input.index('static SOURCE:'):bwm_input.index('#[derive(Clone, Copy)]')]
        for marker in ['fn dispose(', 'pub(super) fn begin(', 'pub(super) fn finish(']:
            program += item(bwm_input, marker) + '\n'
        program += r'''
}
struct Stack;
impl Stack { fn focus(&self) -> Option<u64> { Some(9) } }
fn main() {
    let event = Event { serial: 42, device: 7, kind: kind::KEY, code: 30, value: 2,
        ..Event::default() };
    {
        let mut s = STATE.lock();
        s.grab = Some(Grab { reader: 7, devices: vec![], holder: 1, epoch: 1 });
        s.ledger.push_back(Row { event, reader: Some(7), outcome: None });
    }
    assert!(ledger().is_empty(), "pending rows must not masquerade as terminal");
    bwm_input::initialize();
    let mut timing = timing::Timing::new(); let stack = Stack;
    timing.key(Event { value: 1, ..event }, stack.focus(), false, 0);
    for event in [event] {
        bwm_input::begin(event);
'''
        program += suppress.replace('input::finish()', 'bwm_input::finish()')
        program += r'''
        panic!("a device repeat reached ordinary key delivery");
    }
    assert_eq!(ledger(), "42 wm-handled 0 none\n");
    assert_eq!(STATE.lock().ledger.len(), 1);
    assert!(STATE.lock().derivations.is_empty());
    let first = timing.due(500_000_000, stack.focus());
    assert_eq!(first.len(), 1, "device repeat must not reset the UI deadline");
    assert_eq!((first[0].source.serial, first[0].derived), (42, 2));
}
'''
        with tempfile.TemporaryDirectory(prefix='bwm-repeat-ledger-') as directory:
            temp = Path(directory)
            rustc = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021']
            subprocess.run(rustc + ['--crate-name', 'input_abi', '--crate-type', 'rlib',
                str(ROOT / 'libs/input-abi/src/lib.rs'), '--out-dir', str(temp)], check=True)
            (temp / 'check.rs').write_text(program)
            subprocess.run(rustc + ['-Adead_code', str(temp / 'check.rs'), '--extern',
                f'input_abi={temp / "libinput_abi.rlib"}', '-o', str(temp / 'check')], check=True)
            subprocess.run([str(temp / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
