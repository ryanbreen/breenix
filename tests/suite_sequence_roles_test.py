"""Execute the production sequence with controlled process/syscall boundaries."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

STUBS = r'''
// Compile the full production module; these tests exercise only the role entry points.
#![allow(dead_code)]
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
static PID: AtomicU64 = AtomicU64::new(40);
static SWEEPS: AtomicUsize = AtomicUsize::new(0);
mod types { #[derive(Clone, Copy)] pub struct Fd; impl Fd {
    pub const STDOUT: Self = Self;
    pub fn from_raw(_: u64) -> Self { Self }
} }
mod io {
    pub fn write(_: crate::types::Fd, b: &[u8]) -> Result<usize, ()> { Ok(b.len()) }
    pub fn close(_: crate::types::Fd) -> Result<(), ()> { Ok(()) }
}
mod process {
    pub const WNOHANG: u32 = 1;
    pub struct Pid(u64); impl Pid { pub fn raw(&self) -> u64 { self.0 } }
    pub fn getpid() -> Result<Pid, ()> { Ok(Pid(crate::PID.load(crate::Ordering::Relaxed))) }
    pub fn waitpid(_: i32, _: &mut i32, _: u32) -> Result<Pid, ()> { Err(()) }
    pub fn yield_now() -> Result<(), ()> { Ok(()) }
    pub fn execv(_: &[u8], _: *const *const u8) -> Result<(), ()> { Err(()) }
}
mod signal {
    pub const ITIMER_REAL: i32 = 0;
    pub const SIGALRM: i32 = 14;
    pub const SIGKILL: i32 = 9;
    pub const SIGSTOP: i32 = 19;
    pub const SIG_SETMASK: i32 = 2;
    pub struct Timeval { pub tv_sec: i64, pub tv_usec: i64 }
    pub struct Itimerval { pub it_interval: Timeval, pub it_value: Timeval }
    #[derive(Default)] pub struct Sigaction;
    impl Sigaction { pub fn new(_: extern "C" fn(i32)) -> Self { Self } }
    pub fn sigaction(_: i32, _: Option<&Sigaction>, _: Option<&mut Sigaction>) -> Result<(), ()> { Ok(()) }
    pub fn sigprocmask(_: i32, _: Option<&u64>, _: Option<&mut u64>) -> Result<(), ()> { Ok(()) }
    pub fn kill(pid: i32, _: i32) -> Result<(), ()> {
        assert_ne!(pid, -1, "scoped sequence would kill non-descendants"); Ok(())
    }
    pub fn setitimer(_: i32, t: &Itimerval, _: Option<&mut Itimerval>) -> Result<(), ()> {
        let _ = t.it_interval.tv_sec + t.it_interval.tv_usec + t.it_value.tv_sec + t.it_value.tv_usec; Ok(())
    }
}
mod time {
    pub struct Ts; impl Ts { pub fn as_nanos(&self) -> i128 { 1 } }
    pub fn now_monotonic() -> Result<Ts, ()> { Ok(Ts) }
    pub fn sleep_ms(_: u64) -> Result<(), ()> { Ok(()) }
}
mod harness {
    pub fn hostkey_method() -> Option<&'static str> { None }
    pub mod descendants {
        pub fn sweep_descendants() -> Option<&'static str> {
            crate::SWEEPS.fetch_add(1, crate::Ordering::Relaxed); None
        }
    }
    pub mod sequence {
'''
CHECKS = r'''
        #[test] fn private_list_and_baseline() {
            let root = std::env::temp_dir().join(format!("sequence-role-{}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            std::env::set_var("BREENIX_SUITE_SEQUENCE_DIR", &root);
            std::fs::write(root.join("suite-sequence"), "runner-fixture,sequence-sentinel\n").unwrap();
            assert_eq!(read("runner-fixture"), Some((vec!["runner-fixture".into(), "sequence-sentinel".into()], 0)));
            assert!(skipped(root.join("suite-baseline").to_str().unwrap()));
            std::fs::write(root.join("suite-baseline"), "f 644 0 0 4 123 /test\n").unwrap();
            assert_eq!(load_baseline().unwrap().len(), 1);
            // PID 1 ignores the same hostile environment, including the skip path.
            crate::PID.store(1, crate::Ordering::Relaxed);
            assert!(!skipped(root.join("suite-baseline").to_str().unwrap()));
            crate::PID.store(40, crate::Ordering::Relaxed);
            std::env::remove_var("BREENIX_SUITE_SEQUENCE_DIR");
            std::fs::remove_dir_all(root).unwrap();
        }
        #[test] fn sequence_reap_uses_the_descendant_sweep() {
            assert!(reap_all());
            assert_eq!(crate::SWEEPS.load(crate::Ordering::Relaxed), 1);
        }
    }
}
'''

class SequenceRoles(unittest.TestCase):
    def test_scoped_completion_and_standard_transcript(self):
        from suite_runner_roles_test import item
        source = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        run = item(source, 'pub fn run_with_finalizer(')
        completion = run[run.index('        finalize();'):run.index('        feed.finish(')]
        setup = item(source, 'fn start_scoped_runner(')
        harness = r'''use std::sync::{Mutex, atomic::{AtomicI32, Ordering}};
static TRANSCRIPT_FD: AtomicI32 = AtomicI32::new(1);
static FLAGS: AtomicI32 = AtomicI32::new(1);
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
#[derive(Clone,Copy)] struct Fd(u64); impl Fd { fn from_raw(n:u64)->Self {Self(n)} }
mod io {
    use super::*;
    pub mod fcntl_cmd { pub const F_GETFD: i32=1; pub const F_SETFD: i32=2; }
    pub mod fd_flags { pub const FD_CLOEXEC: i32=1; }
    pub fn fcntl(fd:Fd, cmd:i32, flags:i64)->Result<i64,String> {
        assert_eq!(fd.0, std::env::var("BREENIX_SUITE_TRANSCRIPT_FD").unwrap().parse::<u64>().unwrap());
        if cmd == fcntl_cmd::F_SETFD { FLAGS.store(flags as i32,Ordering::Relaxed); }
        Ok(i64::from(FLAGS.load(Ordering::Relaxed)))
    }
}
mod process {
    pub fn set_child_subreaper(value:bool)->Result<(),String> {assert!(value);Ok(())}
    pub fn exit(code:i32)->! {panic!("exit:{code}")}
}
mod sequence { pub fn finish() { super::EVENTS.lock().unwrap().push("finish".into()); } }
fn emit(line:&str) {EVENTS.lock().unwrap().push(line.into());}
struct Counts { failed:usize }
struct Suite { id:&'static str }
impl Suite { fn complete(&self) {
    let scoped=true; let counts=Counts{failed:0}; let reclaimed=true;
    let place=Some(());
    let _ = (&place,self.id,emit as fn(&str),sequence::finish as fn()); // Inputs also exist in the old early-exit branch.
    let finalize=|| EVENTS.lock().unwrap().push("finalize".into());
''' + completion + r'''
} }
''' + setup + r'''
#[test] fn completion_reaches_the_real_sequence_entry() {
    let result=std::panic::catch_unwind(|| Suite{id:"runner-fixture"}.complete());
    assert!(result.is_err());
    assert_eq!(*EVENTS.lock().unwrap(), ["finalize","SUITE_SEQUENCE READY runner-fixture","finish"]);
}
#[test] fn stdout_survives_exec_private_transcript_does_not() {
    std::env::set_var("BREENIX_SUITE_TRANSCRIPT_FD","1");
    start_scoped_runner().unwrap(); assert_eq!(FLAGS.load(Ordering::Relaxed)&1,0);
    std::env::set_var("BREENIX_SUITE_TRANSCRIPT_FD","3");
    start_scoped_runner().unwrap(); assert_eq!(FLAGS.load(Ordering::Relaxed)&1,1);
}
'''
        with tempfile.TemporaryDirectory() as scratch:
            path, binary = Path(scratch) / 'test.rs', Path(scratch) / 'test'
            path.write_text(harness)
            result = subprocess.run(['rustc', '--edition=2021', '-Dwarnings', '--test', str(path), '-o', str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(binary), '--test-threads=1'], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_production_sequence_roles(self):
        source = (ROOT / 'libs/libbreenix/src/suite_sequence.rs').read_text()
        # Comments use inner module docs; the actual implementation is unmodified.
        source = '\n'.join(line for line in source.splitlines() if not line.startswith('//!'))
        with tempfile.TemporaryDirectory() as scratch:
            source_path = Path(scratch) / 'test.rs'
            source_path.write_text(STUBS + source + CHECKS)
            binary = Path(scratch) / 'test'
            result = subprocess.run(['rustc', '--edition=2021', '-Dwarnings', '--test', str(source_path), '-o', str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            # The host invokes read/load and reap entry points, without calling prepare or walking /.
            result = subprocess.run([str(binary), '--test-threads=1'], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

if __name__ == '__main__':
    unittest.main()
