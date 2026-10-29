"""G12: retain startup before suite entry and late evidence after reclamation."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class RuntimeUpload(unittest.TestCase):
    def test_gpu_entry_runs_startup_and_final_slots_once_in_order(self):
        source = (ROOT / 'userspace/programs/src/suite_gpu.rs').read_text()
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertIn('.with_finalizer(final_upload)', source)
        fixture = r'''
use std::sync::Mutex;
static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());
mod libbreenix { pub mod gpu {
    pub mod abi { pub mod backend { pub const GK208:u32 = 6; } }
    pub struct Caps { pub backend:u32 }
    pub fn caps()->(i64,Caps) { EVENTS(); (0,Caps{backend:6}) }
    #[allow(non_snake_case)] fn EVENTS() {super::super::EVENTS.lock().unwrap().push("caps");}
} }
mod gt710_artifacts {
    pub fn upload()->Result<(), &'static str> {
        assert_eq!(*super::EVENTS.lock().unwrap(), vec!["caps"]);
        super::EVENTS.lock().unwrap().push("startup-upload");Ok(())
    }
    pub fn upload_final(resources:Option<&super::libbreenix::gpu::Caps>)->Result<(), &'static str> {
        assert!(resources.is_some());
        assert_eq!(*super::EVENTS.lock().unwrap(), vec!["caps","startup-upload","cases","reclaim","caps"]);
        super::EVENTS.lock().unwrap().push("final-upload");Ok(())
    }
}
struct Suite { finalizer:Option<fn()> }
static SUITE:Suite = Suite{finalizer:None}.with_finalizer(final_upload);
impl Suite {
BUILDER
RUN
    fn run_with_finalizer(&self, finalize:impl FnOnce())->! {
        assert_eq!(*EVENTS.lock().unwrap(), vec!["caps","startup-upload"]);
        EVENTS.lock().unwrap().extend(["cases","reclaim"]);
        finalize();
        assert_eq!(EVENTS.lock().unwrap().last(),Some(&"final-upload"));
        std::process::exit(0)
    }
}
FINAL
ENTRY
fn main() { producer_main(); }
'''
        entry = item(source, 'fn main()').replace('fn main()', 'fn producer_main()')
        entry = entry.replace('#[cfg(target_arch="x86_64")]', '')
        fixture = fixture.replace('ENTRY', entry).replace('FINAL', item(source, 'fn final_upload()'))
        fixture = fixture.replace('BUILDER', item(runner, 'pub const fn with_finalizer('))
        fixture = fixture.replace('RUN', item(runner, 'pub fn run(&self)'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'entry.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'entry.rs'), '-o', str(path / 'entry')], check=True)
            subprocess.run([str(path / 'entry')], check=True)


if __name__ == '__main__': unittest.main()
