"""PRESENT must wait at the native submission boundary during a stalled core."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class Stall(unittest.TestCase):
    def test_valid_present_waits_before_backend_build_and_invalid_handles_still_fail(self):
        source=item((ROOT/'kernel/src/gpu/display.rs').read_text(),'pub(super) fn present_as(')
        code=source[source.index('        if !owner(tgid)',source.index('let sent =')):source.index('        // Count admitted display work')]
        fixture=r'''
use std::collections::BTreeMap;
const EPERM:i64=1;const EINVAL:i64=22;const EIO:i64=5;
mod kind{pub const TARGET:u32=1;}mod limits{pub const IN_FLIGHT:usize=4;}
#[derive(PartialEq)]enum CtxState{Live}
struct Context{state:CtxState,device_alive:bool,dropped:bool,inflight:Vec<()>,busy:bool}
struct Resource{kind:u32,scanout:bool,lost:bool,dropped:bool,width:u32,height:u32,writers:u32,invalid:bool,last:Option<()>}
struct Core{stalled:bool,display:Display,contexts:BTreeMap<u32,Context>,resources:BTreeMap<u32,Resource>}
struct Display{changing:bool}
impl Core{fn ctx_of(&self,_:u64,_:u64)->Option<u32>{Some(1)}fn res_of(&self,_:u64,h:u64)->Option<u32>{(h==1).then_some(1)}}
fn owner(_:u64)->bool{true}struct Arg{context:u64,target:u64}
fn candidate(c:&mut Core,a:Arg)->Option<Result<(),i64>>{let tgid=7;CODE
 let _=(width,height);Some(Ok(()))}
#[test]fn a_stalled_native_present_is_killably_waiting_before_build(){
 let mut c=Core{stalled:true,display:Display{changing:false},contexts:BTreeMap::from([(1,Context{state:CtxState::Live,device_alive:true,dropped:false,inflight:vec![],busy:false})]),resources:BTreeMap::from([(1,Resource{kind:1,scanout:true,lost:false,dropped:false,width:64,height:64,writers:0,invalid:false,last:Some(())})])};
 assert_eq!(candidate(&mut c,Arg{context:1,target:1}),None,"PRESENT reached native 64x64 geometry refusal instead of blocking");
 assert_eq!(candidate(&mut c,Arg{context:1,target:2}),Some(Err(EINVAL)));
 c.stalled=false;assert_eq!(candidate(&mut c,Arg{context:1,target:1}),Some(Ok(())));
}
'''.replace('CODE',code)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'stall.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'stall.rs'),'-o',str(p/'stall')],check=True)
            subprocess.run([str(p/'stall'),'--nocapture'],check=True)
if __name__=='__main__':unittest.main()
