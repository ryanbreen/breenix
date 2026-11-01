"""Check sparse native timestamp packet extents and device-written samples."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class GpuTime(unittest.TestCase):
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
