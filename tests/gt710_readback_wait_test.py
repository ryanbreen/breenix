"""Native readback diagnostics retain split costs and the targeted case's rows."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReadbackWait(unittest.TestCase):
    def test_split_and_case_buffer_bound(self):
        source = r'''
extern crate alloc;
#[path="TIMING"] pub mod timing;
fn main() {
 assert!(!timing::collecting(false,false));
 let mut split=timing::ReadbackSplit::default();
 split.wait(100,5_000_100);split.wait(6_000_000,6_100_000);
 split.copy(6_100_000,6_120_000);split.copy(6_120_000,6_150_000);
 assert_eq!(split.wait_ns,5_100_000);assert_eq!(split.copy_ns,50_000);
 let mut r=timing::Recorder::new();r.begin_draw(2);
 // 161810 retained 16,384 ordinary rows and dropped 7,451: 23,835 total.
 // Keep at least a quarter of that count again as headroom, plus split records.
 assert!(timing::MAX_ROWS>=29_794);
 for n in 0..29_794 {r.finish(2,1,7,40_000_000_000+n,40_000_000_001+n);}
 for n in 0..3_000 {
  r.readback([2,1,40_000_000_000+n,5_200_000,split.wait_ns,split.copy_ns,4,2]);
  r.blocked([40_000_000_000+n,1,2,40_001_000_000+n,5_000_000,8000,8001]);
 }
 r.sample_vblank(1,1);
 let text=r.drain(1).unwrap();
 assert!(text.contains("\"dropped\":0"));
 assert!(text.contains("[2,1,40000000000,5200000,5100000,50000,4,2]"));
 assert!(text.contains("[40000000000,1,2,40001000000,5000000,8000,8001]"));
 assert_eq!(text.matches(",7,").count(),29_794);
 // G12 final slot is still 2 MiB; leave 128 KiB for census/heap evidence.
 assert!(text.len()<(2<<20)-(128<<10),"split record export exceeds final slot: {}",text.len());
 assert!(r.drain(2).is_none());
}
'''.replace('TIMING', str(ROOT / 'kernel/src/gpu/gk208/timing.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-readback-wait-') as tmp:
            p = Path(tmp)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
