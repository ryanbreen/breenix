"""Exercise the terminal fork-child setup with init's ignored HUP disposition."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class HangupInheritance(unittest.TestCase):
    def test_new_terminal_session_restores_hangup_before_exec(self):
        source = (ROOT / 'userspace/programs/src/bterm.rs').read_text()
        child = item(source[source.index('fn spawn_child('):], 'Ok(ForkResult::Child) =>')
        child = child[child.index('{') + 1:-1]
        harness = r'''
unsafe extern "C" {
 fn signal(number: i32, handler: usize) -> usize;
 fn kill(pid: i32, number: i32) -> i32;
 fn getpid() -> i32;
}
#[derive(Clone, Copy)]
pub struct Fd(u64);
impl Fd { pub fn from_raw(fd: u64) -> Self { Self(fd) } pub fn raw(self) -> u64 { self.0 } }
pub mod io {
 pub fn close(_: crate::Fd) -> Result<(), String> { Ok(()) }
 pub fn dup2(_: crate::Fd, _: crate::Fd) -> Result<(), String> { Ok(()) }
}
pub mod fs {
 pub const O_RDWR: u32 = 2;
 pub fn open(_: &str, _: u32) -> Result<crate::Fd, String> { Ok(crate::Fd(8)) }
}
pub mod libbreenix {
 pub mod process { pub fn exit(code: i32) -> ! { std::process::exit(code) } }
 pub mod termios { pub fn set_controlling_terminal(_: crate::Fd) -> Result<(), String> { Ok(()) } }
 pub mod signal {
  pub const SIGHUP: i32 = 1;
  pub struct Sigaction { handler: usize }
  impl Sigaction { pub fn default_action() -> Self { Self { handler: 0 } } }
  pub fn sigaction(sig: i32, action: Option<&Sigaction>, _: Option<&mut Sigaction>) -> Result<(), String> {
   assert_eq!(sig, SIGHUP);
   unsafe { crate::signal(sig, action.unwrap().handler); }
   Ok(())
  }
 }
}
fn setsid() -> Result<(), String> { Ok(()) }
fn execv(_: &[u8], _: *const *const u8) -> Result<(), String> {
 // A second tab retains the first master, so EOF cannot rescue an ignored HUP.
 // Send the real host signal at the production exec boundary instead.
 unsafe { assert_eq!(kill(getpid(), 1), 0); }
 Err("HUP was ignored".into())
}
fn main() {
 // PID-1 init's detach_console intentionally leaves this inherited disposition.
 unsafe { signal(1, 1); }
 let master_fd = Fd(7);
 let slave_path_str = "/dev/pts/0\0";
 let path = b"/bin/bsh\0";
 let argv: Vec<*const u8> = vec![path.as_ptr(), std::ptr::null()];
 // CHILD
}
'''.replace('// CHILD', child)
        with tempfile.TemporaryDirectory(prefix='bterm-hup-') as tmp:
            path = Path(tmp)
            (path / 'hup.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'hup.rs'), '-o', str(path / 'hup')], check=True)
            result = subprocess.run([str(path / 'hup')], capture_output=True, text=True)
            self.assertEqual(result.returncode, -1,
                             'terminal child ignored SIGHUP at exec boundary: ' + str(result.returncode))


if __name__ == '__main__':
    unittest.main()
