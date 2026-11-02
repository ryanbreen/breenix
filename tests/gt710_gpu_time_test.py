"""Check sparse native timestamp packet extents and device-written samples."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class GpuTime(unittest.TestCase):
    def test_ptimer_read_is_inside_serialized_cpu_clock_brackets(self):
        source = r'''
#[allow(dead_code)] // Import the complete timestamp API for the clock IO check.
#[path="MODULE"] mod gpu_time;
fn main(){
 use std::cell::{Cell,RefCell};
 let events=RefCell::new(Vec::new());let calls=Cell::new(0);let reads=Cell::new(0);
 let sample=gpu_time::clock_sample(
  ||{events.borrow_mut().push(2);let n=calls.get();calls.set(n+1);100+n*100},
  |address|{events.borrow_mut().push(address);let n=reads.get();reads.set(n+1);[3,123,3][n]},
  ||events.borrow_mut().push(1));
 assert_eq!(sample,Some([100,(3u64<<32)+123,200]));
 assert_eq!(*events.borrow(),[1,2,1,0x9410,0x9400,0x9410,1,2,1]);
}
'''.replace('MODULE', str(ROOT / 'kernel/src/gpu/gk208/gpu_time.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-clock-order-') as directory:
            p = Path(directory)
            (p / 'order.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                           str(p / 'order.rs'), '-o', str(p / 'order')], check=True)
            subprocess.run([str(p / 'order')], check=True)

    def test_cpu_clock_brackets_and_complete_request_aggregation(self):
        source = r'''
#[path="MODULE"] mod gpu_time;
fn main() {
 let q=gpu_time::end_query(0x02000000,256).unwrap();
 assert_eq!(q,[0x200406c0,0,0x02000830,256,0x0000f002]);
 let mut stats=[0i128;18];
 // GPU clock is CPU clock + 9000. The read brackets CPU [1000,1020].
 // Submit=100, GPU begin=9200, draw end=9300, parked=9400, end=9500.
 let row=[256,1400,9200,9300,9400,1];
 gpu_time::accumulate(&mut stats,row,9500,[100,600,1000,10010,1020]);
 assert_eq!(&stats[..10],&[1,1,90,110,100,100,100,90,110,20]);
 for _ in 0..40_000 {gpu_time::accumulate(&mut stats,row,9500,[100,600,1000,10010,1020]);}
 assert_eq!(stats[0],40_001);assert_eq!(stats[1],40_001);
 let mut bad=row;bad[5]=0;
 gpu_time::accumulate(&mut stats,bad,9500,[100,600,1000,10010,1020]);
 assert_eq!(stats[0],40_002);assert_eq!(stats[1],40_001);
 // Existing packet/decode helpers stay covered by the companion check.
 assert!(gpu_time::sampled(256));assert_eq!(gpu_time::offset(256,0).unwrap(),1216);
 assert_eq!(gpu_time::query(0x02000000,256,0).unwrap()[4],0xf002);
 assert_eq!(gpu_time::decode(256,1,[[256,0,1,0];3])[5],1);
 assert_eq!(gpu_time::clock_sample(||1,|_|0,||{}),Some([1,0,1]));
}
'''.replace('MODULE', str(ROOT / 'kernel/src/gpu/gk208/gpu_time.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-gpu-clock-') as directory:
            p = Path(directory)
            (p / 'clock.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                           str(p / 'clock.rs'), '-o', str(p / 'clock')], check=True)
            subprocess.run([str(p / 'clock')], check=True)

    def test_query_packets_and_sample_provenance(self):
        module = ROOT / 'kernel/src/gpu/gk208/gpu_time.rs'
        self.assertTrue(module.exists(), 'retain device timing separately from host polling')
        source = r'''
#[path="MODULE"] mod gpu_time;
fn main() {
 assert!(!gpu_time::sampled(0)); assert!(gpu_time::sampled(256));
 assert!(!gpu_time::sampled(255));
 for sequence in 1..=1024 {
  for stage in 0..3 {
   let offset=gpu_time::offset(sequence,stage).unwrap();
   assert_eq!(offset,1024+((sequence-1)%4) as u64*64+stage as u64*16);
   assert!(offset+16<=1280);
   let q=gpu_time::query(0x02000000,sequence,stage).unwrap();
   assert_eq!(q,[0x200406c0,0,0x02000000+offset as u32,sequence,0x0000f002]);
  }
 }
 assert!(gpu_time::query(0x02000000,0,0).is_err());
 assert!(gpu_time::query(0x02000000,256,3).is_err());
 let raw=[[256,0,100,1],[256,0,150,1],[256,0,250,1]];
 assert_eq!(gpu_time::decode(256,1400,raw),[256,1400,(1<<32)+100,(1<<32)+150,(1<<32)+250,1]);
 for index in 0..3 {
  let mut stale=raw;stale[index][0]=255;
  assert_eq!(gpu_time::decode(256,1400,stale)[5],0,"stale sample must remain visibly invalid");
 }
 let mut backwards=raw;backwards[2][2]=149;
 assert_eq!(gpu_time::decode(256,1400,backwards)[5],0);
 assert_eq!(gpu_time::decode(256,1400,[[0;4];3])[5],0);
 assert_eq!(gpu_time::end_query(0x02000000,256).unwrap()[4],0xf002);
 let mut stats=[0;18];gpu_time::accumulate(&mut stats,[1,1,1,2,3,1],4,[1,5,6,7,8]);
 assert_eq!(stats[0],1);
 assert_eq!(gpu_time::clock_sample(||1,|_|0,||{}),Some([1,0,1]));
}
'''.replace('MODULE', str(module))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-gpu-time-') as directory:
            p = Path(directory)
            (p / 'time.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                           str(p / 'time.rs'), '-o', str(p / 'time')], check=True)
            subprocess.run([str(p / 'time')], check=True)


if __name__ == '__main__':
    unittest.main()
