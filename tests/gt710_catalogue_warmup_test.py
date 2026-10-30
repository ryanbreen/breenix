"""Run the core's real warm-up mask and completion accounting on the host."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class CatalogueWarmup(unittest.TestCase):
    def test_only_the_last_native_release_warms_each_rendering_pipeline(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        fixture = r'''
use std::collections::BTreeMap;
mod encode { pub const ALL_WARM:u32=(1<<16)-1; }
#[derive(PartialEq)] enum CtxState { Live, Quarantined }
struct Ctx { state:CtxState, warm:u32, warm_failures:u32, native_warm_scratch:Option<()> }
struct WarmStep { ctx:u32, pipeline:u32, left:u32, refused:bool, lost:bool }
struct Core { contexts:BTreeMap<u32,Ctx>, warm_step:Option<WarmStep> }
fn notify() {}
// MASK
impl Core {
 fn settle(&mut self, _:u32) {}
 // RETURNED
}
fn main() {
 let mut c=Core { contexts:BTreeMap::from([(1,Ctx { state:CtxState::Live, warm:0, warm_failures:0, native_warm_scratch:Some(()) })]),warm_step:None };
 assert_eq!(catalogue_mask(),(1<<gpu_abi::pipeline::PROBE_INT)-1);
 for pipe in 0..gpu_abi::pipeline::PROBE_INT {
  let snapshot=gpu_abi::draw::blended(pipe);
  c.warm_step=Some(WarmStep {ctx:1,pipeline:pipe,left:1+u32::from(snapshot),refused:false,lost:false});
  if snapshot {
   c.warm_returned(Some(true));
   assert_eq!(c.contexts[&1].warm & (1<<pipe),0,"snapshot is not a draw release");
   assert!(c.warm_step.is_some());
  }
  c.warm_returned(Some(true));
  assert_ne!(c.contexts[&1].warm & (1<<pipe),0);
  assert!(c.warm_step.is_none());
 }
 assert_eq!(2*c.contexts[&1].warm.count_ones(),32);
 assert!(c.contexts[&1].native_warm_scratch.is_none(),"completed warm-up must release its scratch charge");
 assert_eq!(c.contexts[&1].warm & (1<<gpu_abi::pipeline::PROBE_INT),0);
 for result in [None,Some(false)] {
  c.contexts.get_mut(&1).unwrap().warm=0;
  c.warm_step=Some(WarmStep {ctx:1,pipeline:gpu_abi::pipeline::OVER,left:2,refused:false,lost:false});
  c.warm_returned(result);
  c.warm_returned(Some(true));
  assert_eq!(c.contexts[&1].warm,0,"a refused or reset step cannot warm a program");
 }
}
'''.replace('// MASK', item(source, 'fn catalogue_mask(')).replace(
            '// RETURNED', item(source, '    fn warm_returned(').replace(
                '#[cfg(target_arch = "x86_64")]', ''))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            abi = directory / 'libgpu_abi.rlib'
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                '--crate-type=lib', '--crate-name=gpu_abi', '--cfg', 'feature="alloc"',
                str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(abi)], check=True)
            path = directory / 'warm.rs'
            path.write_text(fixture)
            binary = directory / 'warm'
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                '--extern', f'gpu_abi={abi}', str(path), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

    def test_native_first_probe_adds_only_its_completed_program_pair(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        fixture = r'''
#![allow(dead_code)] // Ctx exposes only the fields used by its extracted method.
mod abi { pub mod backend { pub const GK208:u32=6; } }
mod dev { pub fn is_svga()->bool { false } pub fn backend()->u32 { 6 } }
struct Objects;
impl Objects { fn probe_programs(&self)->u32 { 0 } }
struct Ctx { warm:u32, native_probe_mask:u32, objects:Objects, dx:Objects }
impl Ctx { // WARMED
}
fn main() {
 let mut ctx=Ctx { warm:0xffff,native_probe_mask:0,objects:Objects,dx:Objects };
 assert_eq!(ctx.warmed(),32);
 ctx.native_probe_mask=1;
 assert_eq!(ctx.warmed(),34,"first completed native PROBE must warm its program pair");
 ctx.native_probe_mask|=1;
 assert_eq!(ctx.warmed(),34,"repeat PROBE must not count another pair");
 ctx.native_probe_mask|=1<<24;
 assert_eq!(ctx.warmed(),36);
}
'''.replace('// WARMED', item(source, '    fn warmed(').replace(
            '#[cfg(target_arch = "x86_64")]', ''))
        # Exercise the native branch even when the test host is an ARM Mac.
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'probe.rs'
            path.write_text(fixture)
            binary = Path(directory) / 'probe'
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                str(path), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == '__main__':
    unittest.main()
