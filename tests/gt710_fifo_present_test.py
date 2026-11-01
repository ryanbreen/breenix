"""G26: execute native PRESENT admission and scanout write exclusion on the host."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item

class FifoPresent(unittest.TestCase):
    def test_present_waits_for_the_earlier_latch(self):
        source = item((ROOT / 'kernel/src/gpu/display.rs').read_text(), 'pub(super) fn present_as(')
        code = source[source.index('        if !owner(tgid)', source.index('let sent =')):source.index('        // Count admitted display work')]
        harness = r'''
use std::collections::BTreeMap;
const EPERM:i64=1;const EINVAL:i64=22;const EIO:i64=5;
mod abi{pub mod backend{pub const GK208:u32=6;}}
mod dev{pub fn backend()->u32{6}}
mod kind{pub const TARGET:u32=1;}mod limits{pub const IN_FLIGHT:usize=4;}
#[derive(PartialEq)]enum CtxState{Live}
struct Context{state:CtxState,device_alive:bool,dropped:bool,inflight:Vec<()>,busy:bool}
struct Resource{kind:u32,scanout:bool,lost:bool,dropped:bool,width:u32,height:u32,writers:u32,invalid:bool,last:Option<()>}
struct Core{stalled:bool,display:Display,contexts:BTreeMap<u32,Context>,resources:BTreeMap<u32,Resource>}
struct Display{changing:bool,outstanding:u32}
impl Core{fn ctx_of(&self,_:u64,_:u64)->Option<u32>{Some(1)}fn res_of(&self,_:u64,h:u64)->Option<u32>{(h==1).then_some(1)}}
fn owner(_:u64)->bool{true}struct Arg{context:u64,target:u64}
fn candidate(c:&mut Core,a:Arg)->Option<Result<(),i64>>{let tgid=7;CODE
 let _=(width,height);Some(Ok(()))}
#[test]fn second_present_waits_until_the_first_flip_latches(){
 let mut c=Core{stalled:false,display:Display{changing:false,outstanding:1},contexts:BTreeMap::from([(1,Context{state:CtxState::Live,device_alive:true,dropped:false,inflight:vec![],busy:false})]),resources:BTreeMap::from([(1,Resource{kind:1,scanout:true,lost:false,dropped:false,width:1280,height:800,writers:0,invalid:false,last:Some(())})])};
 assert_eq!(candidate(&mut c,Arg{context:1,target:1}),None,"second PRESENT entered before first latch");
 c.display.outstanding=0;
 assert_eq!(candidate(&mut c,Arg{context:1,target:1}),Some(Ok(())));
}
'''.replace('CODE', code)
        self.run_rust(harness)

    def test_submit_waits_for_displayed_and_pending_targets(self):
        source = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        # Before G26 the production path has no scanout write exclusion.
        method = (item(source, 'pub(super) fn write_blocked(')
                  if 'pub(super) fn write_blocked(' in source else
                  'pub(super) fn write_blocked(_: &Core, _: &[u32])->bool {false}')
        harness = r'''
use std::collections::BTreeSet;
mod abi{pub mod backend{pub const GK208:u32=6;}}
mod dev{pub fn backend()->u32{6}}
struct Display{holds:BTreeSet<u32>}
struct Core{display:Display}
mod display{use super::*;METHOD}
#[test]fn submit_cannot_write_displayed_or_pending_storage(){
 let mut c=Core{display:Display{holds:[71,72].into_iter().collect()}};
 assert!(display::write_blocked(&c,&[71]),"displayed A was writable before B latched");
 assert!(display::write_blocked(&c,&[72]),"pending B was writable before latch");
 assert!(!display::write_blocked(&c,&[73]),"disjoint C must remain writable");
 c.display.holds.remove(&71); // B's completion notifier ends A's hold.
 assert!(!display::write_blocked(&c,&[71]));
 assert!(display::write_blocked(&c,&[72]),"B remains displayed after its own latch");
}
'''.replace('METHOD', method)
        self.run_rust(harness)
        submit = item((ROOT / 'kernel/src/gpu/mod.rs').read_text(), 'fn submit_stream(')
        self.assertIn('display::write_blocked(c, &written)', submit)

    def run_rust(self, harness):
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        # Extracted pre-G26 paths deliberately do not use the pending/backend fields.
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'fifo.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Adead_code', '-Dwarnings', str(p / 'fifo.rs'), '-o', str(p / 'fifo')], check=True)
            subprocess.run([str(p / 'fifo'), '--nocapture'], check=True)

if __name__ == '__main__':
    unittest.main()
