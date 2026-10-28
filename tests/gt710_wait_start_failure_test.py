"""A failed CE child must be reaped and reported while the console continues."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WaitStartFailure(unittest.TestCase):
    def test_failed_begin_returns_error_evidence_without_waiting_for_queue(self):
        source = (ROOT / 'userspace/programs/src/gt710_wait_probe.rs').read_text()
        source = source.replace('//!', '//').replace('std::fs::read_to_string', 'read_probe')
        harness = r'''
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
static CLOCK:AtomicU64=AtomicU64::new(0);
static REAPED:AtomicUsize=AtomicUsize::new(0);
static KILLED:AtomicUsize=AtomicUsize::new(0);
pub mod libbreenix {
    pub mod process {
        #[derive(Clone,Copy)] pub struct Pid(i32);
        impl Pid {pub fn raw(self)->u32 {self.0 as u32}}
        pub enum ForkResult {Child,Parent(Pid)}
        pub fn fork()->Result<ForkResult,&'static str>{Ok(ForkResult::Parent(Pid(7)))}
        pub fn exit(_:i32)->!{panic!("unexpected child branch")}
        pub fn waitpid(pid:i32,status:&mut i32,_:i32)->Result<Pid,&'static str>{
            assert_eq!(pid,7); *status=99<<8;
            crate::REAPED.fetch_add(1,crate::Ordering::Relaxed); Ok(Pid(pid))
        }
    }
    pub mod signal {
        pub const SIGKILL:u32=9;
        pub fn kill(_:i32,_:u32)->Result<(),&'static str>{crate::KILLED.fetch_add(1,crate::Ordering::Relaxed); Ok(())}
    }
    pub mod time {
        pub struct Timespec {pub tv_sec:i64,pub tv_nsec:i64}
        pub fn now_monotonic()->Result<Timespec,&'static str>{
            Ok(Timespec{tv_sec:0,tv_nsec:crate::CLOCK.fetch_add(10_000_000,crate::Ordering::Relaxed) as i64})
        }
        pub fn sleep_ms(_:u64)->Result<(),&'static str>{Ok(())}
    }
}
fn read_probe(path:&str)->std::io::Result<String>{
    assert_eq!(path,"/proc/gt710-ce"); Ok("ready\n".into())
}
SOURCE
fn main(){
    run();
    assert_eq!(REAPED.load(Ordering::Relaxed),1,"failed child not reaped");
    assert_eq!(KILLED.load(Ordering::Relaxed),0,"exited child was killed");
    assert!(CLOCK.load(Ordering::Relaxed)<2_000_000_000,"waited for impossible queue");
    println!("CONSOLE_CONTINUES");
}
'''.replace('SOURCE', source)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'probe.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'probe.rs'),
                                      '-o', str(path / 'probe')], check=True, cwd=ROOT)
            result = subprocess.run([str(path / 'probe')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('GT710_KILLED_WAIT_ERROR', result.stdout)
            self.assertTrue(result.stdout.endswith('CONSOLE_CONTINUES\n'))
            self.assertNotIn('GT710_KILLED_WAIT {', result.stdout)


if __name__ == '__main__':
    unittest.main()
