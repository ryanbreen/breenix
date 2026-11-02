"""Heap snapshots retain costs without implicitly enabling detailed diagnostics."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class TimingRequest(unittest.TestCase):
    def test_detailed_collection_requires_an_explicit_boot_option(self):
        enable = item((ROOT / 'kernel/src/gpu/gk208/timing.rs').read_text(), '    pub fn enable()')
        source = r'''
extern crate alloc;
mod boot{pub mod intent{
 std::thread_local!{static LINE:std::cell::Cell<Option<&'static str>>=const{std::cell::Cell::new(None)};}
 pub fn command_line()->Option<&'static str>{LINE.with(|v|v.get())}
 pub fn set(v:Option<&'static str>){LINE.with(|l|l.set(v));}
}}
#[allow(dead_code)] // The ordinary cost snapshot imports the complete production API.
#[path="COST"]mod cost;
mod runtime{
 static REQUESTED:core::sync::atomic::AtomicBool=core::sync::atomic::AtomicBool::new(false);
 ENABLE
 pub fn clear(){REQUESTED.store(false,core::sync::atomic::Ordering::Release);}
 pub fn requested()->bool{REQUESTED.load(core::sync::atomic::Ordering::Acquire)}
}
fn main(){
 for(line,want)in[(None,false),(Some(""),false),(Some("quiet"),false),(Some("not-gt710-cpu-time"),false),(Some("gt710-cpu-time-extra"),false),(Some("gt710-cpu-time"),true),(Some("quiet gt710-gpu-time extra"),true),(Some("gt710-cpu-time gt710-gpu-time"),true)]{
  runtime::clear();boot::intent::set(line);assert_eq!(boot::intent::command_line(),line);runtime::enable();
  assert_eq!(runtime::requested(),want,"boot option {line:?}: heap observation is not a request for detailed timing");
 }
 let costs=cost::Recorder::new();costs.snapshot();costs.record(cost::Slot::Submit,10,17);let snapshot=costs.snapshot();assert_eq!(&snapshot[..2],&[1,7]);
}
'''.replace('ENABLE', enable).replace('COST', str(ROOT / 'kernel/src/gpu/gk208/cost.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-timing-request-') as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib', '--cfg', 'feature="alloc"', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
