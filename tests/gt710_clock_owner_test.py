"""Clock ownership survives errors without holding the session spin lock."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]

class ClockOwner(unittest.TestCase):
    def test_callback_releases_lock_and_returns_session_on_error(self):
        source = (ROOT/'kernel/src/gpu/gk208/ce/process.rs').read_text()
        method = item(source, 'pub(in crate::gk208) fn with_clock_owner<R>(')
        method = method.replace('pub(in crate::gk208) ', '').replace('super::super::', 'super::')
        harness = r'''
use std::sync::Mutex;
mod gk208 {
 pub struct ConsoleSurface;
 pub mod native {pub struct Evo;}
 pub mod ce {
  struct Registers(usize);
  struct Session {registers:usize,evo:super::native::Evo,console:super::ConsoleSurface,work:Option<()>}
  static SESSION:crate::Mutex<Option<Session>>=crate::Mutex::new(None);
  // Adapt std's poisoning API only; the production function owns all session moves.
  struct Owner;
  impl Owner {fn lock(&self)->std::sync::MutexGuard<'_,Option<Session>>{SESSION.lock().unwrap()}}
  pub fn exercise(){
   *SESSION.lock().unwrap()=Some(Session{registers:7,evo:super::native::Evo,console:super::ConsoleSurface,work:None});
   for fail in [false,true] {
    let result=with_clock_owner(|r,_,_|{
     assert!(SESSION.try_lock().expect("clock wait retained SESSION lock").is_none());
     assert_eq!(r.0,7);
     if fail {Err("PMU timeout")} else {Ok(())}
    });
    assert_eq!(result,if fail {Err("PMU timeout")}else{Ok(())});
    assert!(SESSION.lock().unwrap().is_some());
   }
  }
  // METHOD
 }
}
fn main(){gk208::ce::exercise();}
'''.replace('// METHOD', method.replace('SESSION.lock()', 'Owner.lock()'))
        harness = harness.replace('pub struct ConsoleSurface;', '#[derive(Clone,Copy)] pub struct ConsoleSurface;')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p/'owner.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'owner.rs'),'-o',str(p/'owner')],check=True)
            subprocess.run([str(p/'owner')],check=True)

if __name__ == '__main__':
    unittest.main()
