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
    def test_sync_present_acceptance_does_not_fault_or_complete_early(self):
        source = item((ROOT / 'kernel/src/gpu/mod.rs').read_text(), '    fn reap_all(')
        # The kernel screen uses Tag::Sync; GET acceptance must leave that
        # synchronous request pending until its actual latch completion.
        self.assertIn('Reap::Accepted(Tag::Sync(_)) => {', source)
        arm = source.split('Reap::Accepted(Tag::Sync(_)) => {', 1)[1].split('}', 1)[0]
        self.assertNotIn('fault(', arm)
        self.assertNotIn('sync_ended(', arm)

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
        method = (item(source, 'pub(super) fn write_blocked(')
                  if 'pub(super) fn write_blocked(' in source else
                  'pub(super) fn write_blocked(_: &Core, _: &[u32])->bool {false}')
        accepted = (item(source, 'pub(super) fn present_accepted(')
                    if 'pub(super) fn present_accepted(' in source else
                    'pub(super) fn present_accepted(&mut self, _:u64) {}')
        presented = item(source, 'pub(super) fn presented(')
        harness = r'''
use std::collections::{BTreeMap,BTreeSet};
mod fence_status{pub const OK:u32=0;}
mod abi{pub mod backend{pub const GK208:u32=6;}}
mod dev{pub fn backend()->u32{6}pub fn success()->u32{0x60000000}}
fn notify(){}
#[derive(Clone,Copy)]struct Binding{id:u32,fence:u64,answer:u32}
#[derive(Clone,Copy)]struct Presenting{id:u32,seq:u64,owner:u64}
struct Owner{group:u64,last:Option<Binding>}
struct Display{holds:BTreeSet<u32>,outstanding:u32,sent:u64,accepted:u64,latest_ok:bool,known:bool,binding:Option<Binding>,stack:Vec<Owner>}
struct Resource{holds:u32}struct Proc{presents:u64}
#[derive(Default)]struct Answers{values:BTreeMap<u32,u32>}
impl Answers{fn complete(&mut self,n:u32,a:u32){self.values.insert(n,a);}}
struct Context{ns:u32,done_upto:u32,flushes:Answers}
struct Sub{present:Option<Presenting>,ctx:u32,n:u32}
struct Core{display:Display,resources:BTreeMap<u32,Resource>,procs:BTreeMap<u64,Proc>,subs:BTreeMap<u64,Sub>,contexts:BTreeMap<u32,Context>,released:Vec<u32>}
mod display{use super::*;METHOD}
impl Core{
 fn publish(&mut self,serial:u64,_status:u32,answer:u32){let sub=&self.subs[&serial];let ctx=self.contexts.get_mut(&sub.ctx).unwrap();ctx.done_upto=sub.n;ctx.flushes.complete(sub.n,answer);}
 fn unhold(&mut self,id:u32){self.resources.get_mut(&id).unwrap().holds-=1;self.released.push(id);}
 ACCEPTED
 PRESENTED
}
#[test]fn submit_waits_for_the_next_latch_and_acceptance_completes_the_context_fence(){
 let p=Presenting{id:72,seq:2,owner:1};
 let old=Binding{id:71,fence:1,answer:dev::success()};
 let mut c=Core{display:Display{holds:[71,72].into_iter().collect(),outstanding:1,sent:2,accepted:1,latest_ok:true,known:false,binding:Some(old),stack:vec![Owner{group:1,last:Some(old)}]},resources:BTreeMap::from([(71,Resource{holds:2}),(72,Resource{holds:1})]),procs:BTreeMap::from([(1,Proc{presents:0})]),subs:BTreeMap::from([(17,Sub{present:Some(p),ctx:1,n:2})]),contexts:BTreeMap::from([(1,Context{ns:7,done_upto:0,flushes:Answers::default()})]),released:vec![]};
 c.display.outstanding=0;
 assert!(!display::write_blocked(&c,&[71]),"single-buffered redraw waited for a flip that does not exist");
 c.display.outstanding=1;
 assert!(display::write_blocked(&c,&[71]),"displayed A was writable before B latched");
 assert!(display::write_blocked(&c,&[72]),"pending B was writable before latch");
 assert!(!display::write_blocked(&c,&[73]),"disjoint C must remain writable");
 c.present_accepted(17);
 assert_eq!(c.contexts[&1].done_upto,2,"GET acceptance left the PRESENT fence pending");
 assert_eq!(c.contexts[&1].flushes.values.get(&2),Some(&dev::success()));
 let binding=c.display.binding.unwrap();
 assert_eq!((binding.id,binding.fence,binding.answer),(72,(7<<32)|2,dev::success()));
 assert!(c.display.known);assert_eq!(c.display.outstanding,1);assert_eq!(c.display.accepted,2);
 assert!(c.released.is_empty());assert!(display::write_blocked(&c,&[71]));
 c.presented(p,true,(7<<32)|2,dev::success());
 assert_eq!(c.display.outstanding,0);assert_eq!(c.released,[71,71]);
 assert!(!display::write_blocked(&c,&[71]),"A stays blocked after B's actual latch");
 assert!(!display::write_blocked(&c,&[72]),"single-buffered B cannot redraw after its latch with no flip pending");
}
'''.replace('METHOD', method).replace('ACCEPTED', accepted).replace('PRESENTED', presented).replace('pub(super)', 'pub(crate)')
        self.run_rust(harness)
        submit = item((ROOT / 'kernel/src/gpu/mod.rs').read_text(), 'fn submit_stream(')
        self.assertIn('display::write_blocked(c, &written)', submit)

    def test_readback_pins_the_latest_accepted_binding_before_latch(self):
        source = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        acquire = item(source, '    fn observe() -> Result<Self, i64>' if '    fn observe()' in source else '    fn acquire() -> Result<Self, i64>').replace('fn observe()', 'fn acquire()')
        harness = r'''
mod abi{pub mod backend{pub const GK208:u32=6;}}
mod dev{pub fn backend()->u32{6}}
const EINTR:i64=4;
struct Display{changing:bool,outstanding:u32,known:bool,accepted:u64,sent:u64}
struct Core{display:Display}
thread_local!{static CORE:std::cell::RefCell<Core>=const{std::cell::RefCell::new(Core{display:Display{changing:false,outstanding:1,known:true,accepted:1,sent:2}})};}
fn wait_for<T>(_:Option<u64>,_:bool,f:impl FnOnce(&mut Core)->Option<T>)->Result<T,()>{CORE.with(|c|f(&mut c.borrow_mut()).ok_or(()))}
struct Transition;
impl Transition{ACQUIRE}
#[test]fn a_readback_waits_for_acceptance_then_pins_without_waiting_for_latch(){
 assert!(Transition::acquire().is_err(),"unaccepted UPDATE could change binding inside the readback");
 CORE.with(|c|c.borrow_mut().display.accepted=2);
 assert!(Transition::acquire().is_ok(),"accepted binding readback unnecessarily waited for latch");
 CORE.with(|c|{assert!(c.borrow().display.changing);assert_eq!(c.borrow().display.outstanding,1);});
}
'''.replace('ACQUIRE', acquire)
        self.run_rust(harness)

    def test_mutations_wait_for_the_pending_flip_to_retire(self):
        source = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        acquire = item(source, '    fn acquire() -> Result<Self, i64>')
        harness = r'''
mod abi{pub mod backend{pub const GK208:u32=6;}}
mod dev{pub fn backend()->u32{6}}
const EINTR:i64=4;
struct Display{changing:bool,outstanding:u32,known:bool,accepted:u64,sent:u64}
struct Core{display:Display}
thread_local!{static CORE:std::cell::RefCell<Core>=const{std::cell::RefCell::new(Core{display:Display{changing:false,outstanding:1,known:true,accepted:2,sent:2}})};}
fn wait_for<T>(_:Option<u64>,_:bool,f:impl FnOnce(&mut Core)->Option<T>)->Result<T,()>{CORE.with(|c|f(&mut c.borrow_mut()).ok_or(()))}
struct Transition;
impl Transition{ACQUIRE}
#[test]fn accepted_flip_cannot_be_consumed_behind_the_adapter_by_a_mutation(){
 assert!(Transition::acquire().is_err(),"mode/ownership mutation entered with adapter flip still outstanding");
 CORE.with(|c|{assert!(!c.borrow().display.changing);c.borrow_mut().display.outstanding=0;});
 assert!(Transition::acquire().is_ok());
 CORE.with(|c|assert!(c.borrow().display.changing));
}
'''.replace('ACQUIRE', acquire)
        self.run_rust(harness)
        self.assertIn('Transition::for_mutation(tgid)?', item(source, 'pub(super) fn set_mode('))
        for marker in ['pub(super) fn back(', 'pub(super) fn restore_pending(']:
            self.assertIn('Transition::acquire()?', item(source, marker))

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
