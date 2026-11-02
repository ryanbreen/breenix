"""Short VM invalidation acknowledgements should precede coarse backoff."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]


class VmFastPoll(unittest.TestCase):
    def test_queue_and_ack_arriving_after_a_few_reads_need_no_coarse_pause(self):
        source=r'''
extern crate alloc;
#[allow(dead_code)]#[path="ROOT/kernel/src/gpu/gk208/edid.rs"]mod edid;
#[allow(dead_code)]#[path="ROOT/kernel/src/gpu/gk208/modeset.rs"]mod modeset;
#[path="ROOT/kernel/src/gpu/gk208/vm_control.rs"]mod vm_control;
struct Io{free_after:usize,ack_after:usize,free_reads:usize,ack_reads:usize,issued:bool,delays:Vec<u32>,writes:Vec<(usize,u32)>}
impl modeset::Io for Io{
 fn read(&mut self,reg:usize)->u32{
  if reg!=0x100c80{return 0;}
  if self.issued{self.ack_reads+=1;0x10000|if self.ack_reads>=self.ack_after{0x8000}else{0}}
  else{self.free_reads+=1;if self.free_reads>=self.free_after{0x10000}else{0}}
 }
 fn write(&mut self,r:usize,v:u32){self.writes.push((r,v));if r==0x100cbc{self.issued=true;}}
 fn delay_us(&mut self,us:u32){self.delays.push(us);}
}
fn main(){for free_after in [1,2,5,12]{for ack_after in [1,2,5,12]{
 let mut io=Io{free_after,ack_after,free_reads:0,ack_reads:0,issued:false,delays:Vec::new(),writes:Vec::new()};
 assert_eq!(vm_control::invalidate(&mut io,0x12345000),Ok(0x18000));
 assert!(io.delays.is_empty(),"short queue/ack readiness incurred a 100 us pause: free={free_after} ack={ack_after}");
 assert_eq!(io.writes,[(0x70000,1),(0x100cb8,0x123450),(0x100cbc,0x80000001)]);
}}}
'''.replace('ROOT',str(ROOT))
        priority=['taskpolicy','-b']if sys.platform=='darwin'else[]
        with tempfile.TemporaryDirectory(prefix='gt710-vm-fast-')as directory:
            p=Path(directory);(p/'check.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)


if __name__=='__main__':
    unittest.main()
