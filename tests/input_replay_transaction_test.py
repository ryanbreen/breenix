"""Exercise the production USB replay transaction with a deferred endpoint restart."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReplayTransaction(unittest.TestCase):
    def test_release_waits_for_the_restarted_report_pipe(self):
        source = (ROOT / 'kernel/src/drivers/xhci/keyboard.rs').read_text()
        start = source.index('    fn replay_keyboard_transaction(')
        brace = source.index('{', start)
        depth, end = 1, brace + 1
        while depth:
            depth += (source[end] == '{') - (source[end] == '}')
            end += 1
        method = source[start:end]
        program = r'''
use std::sync::atomic::{AtomicBool,AtomicU32,Ordering};
static RESTART_PENDING: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU32 = AtomicU32::new(0);
pub fn delay(ns: u64) {
    assert!(ns >= 10_000_000);
    RESTART_PENDING.store(false,Ordering::Relaxed);
}
mod input {
    pub mod hardware { #[derive(Clone,Copy)] pub struct Device { pub id:u32 } }
    pub mod control { #[derive(Debug)] pub struct Receipt { pub calls:u32,pub posted:u32,pub ring:u32 } }
}
mod syscall { pub mod errno { pub const ENODEV:i32=19;pub const EIO:i32=5; } }
mod test_control {
    pub struct InputReplay;
    impl InputReplay {
        pub fn begin(device:u32,suppress:bool)->Result<Self,u64> {
            assert_eq!(device,7);assert!(!suppress);Ok(Self)
        }
        pub fn calls(&self)->u32 { crate::CALLS.load(crate::Ordering::Relaxed) }
    }
}
mod spec { pub const POSTED_TDS:usize=4; }
struct Posted;
impl Posted { fn free(&self)->std::iter::Empty<usize> { std::iter::empty() } }
struct Pipe { input:Option<input::hardware::Device>,posted:Posted }
struct Device { keyboard:Option<Pipe> }
struct Controller { devices:Vec<Device> }
impl Controller {
    fn replay_key(&mut self,_report:&[u8;8])->Result<(usize,u32), &'static str> {
        if RESTART_PENDING.load(Ordering::Relaxed) { return Err("endpoint restart still pending"); }
        Ok((0,0))
    }
    fn keyboard_event(&mut self,_index:usize,_event:u32) {
        CALLS.fetch_add(1,Ordering::Relaxed);
        RESTART_PENDING.store(true,Ordering::Relaxed);
    }
'''
        program += method + '\n}\n'
        program += r'''
fn main() {
    let mut c=Controller { devices:vec![Device { keyboard:Some(Pipe {
        input:Some(input::hardware::Device { id:7 }),posted:Posted }) }] };
    let r=c.replay_keyboard_transaction(false).expect("press/release transaction");
    assert_eq!((r.calls,r.posted,r.ring),(2,4,4));
}
'''
        with tempfile.TemporaryDirectory(prefix='input-replay-transaction-') as temp:
            path, binary = Path(temp) / 'check.rs', Path(temp) / 'check'
            path.write_text(program)
            prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            result = subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                str(path), '-o', str(binary)], text=True, capture_output=True, timeout=60)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(binary)], text=True, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
