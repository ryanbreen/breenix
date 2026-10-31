"""Native cost observations are opt-in, fixed-size cumulative call/ns pairs."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
class Cost(unittest.TestCase):
 def test_disabled_default_and_exact_cumulative_phase_costs(self):
  source=f'''extern crate alloc;
#[path="{ROOT/'kernel/src/gpu/gk208/cost.rs'}"]mod cost;
use cost::{{Recorder,Slot}};
fn main(){{
 let r=Recorder::new();assert!(!r.enabled());r.record(Slot::Submit,10,99);
 assert_eq!(r.snapshot(),[0;cost::WORDS]);assert!(r.enabled());
 r.record(Slot::Submit,100,170);r.record(Slot::Submit,200,220);r.record(Slot::Wait,300,340);
 let a=r.snapshot();assert_eq!(&a[..4],&[2,90,1,40]);assert!(a[4..].iter().all(|&v|v==0));
 r.record(Slot::Evo,500,510);let b=r.snapshot();assert_eq!(&b[22..24],&[1,10]);assert!(b[24..].iter().all(|&v|v==0));assert_eq!(&a[..4],&b[..4]);
 for (op,slot) in [(7,Slot::Submit),(8,Slot::Wait),(9,Slot::Read),(12,Slot::Present),(6,Slot::Upload)]{{assert_eq!(cost::slot_for_op(op),Some(slot));}}
 assert_eq!(cost::slot_for_op(0),None);assert_eq!(cost::slot_for_op(u64::MAX),None);
 assert_eq!(gpu_abi::heap_trace::WORDS,32+cost::WORDS);
 assert_eq!(&gpu_abi::heap_trace::NAMES[32..],&cost::NAMES);
}}
'''
  priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'cost.rs').write_text(source)
   subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--cfg','feature="alloc"','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
   subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'cost.rs'),'-o',str(p/'cost')],check=True)
   subprocess.run([str(p/'cost')],check=True)
if __name__=='__main__':unittest.main()
