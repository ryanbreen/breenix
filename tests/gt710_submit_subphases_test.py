"""Compile the production exclusive partition and submit facts without hardware."""
from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SubmitSubphases(unittest.TestCase):
    def test_nested_subphases_bytes_and_slot_waits(self):
        source = r'''
extern crate alloc;
#[path="MODULE"] pub mod timing;
use timing::submit_cost as cost;
use cost::{Split, Phase, Facts, PHASES};
fn main() {
 let s=Split::new(0);
 let phases=[Phase::Admission, Phase::AdmissionPoll, Phase::AdmissionInspect,
   Phase::AdmissionPause, Phase::AdmissionSleep, Phase::Translate,
   Phase::TranslateEncode, Phase::TranslateValidate, Phase::TranslateEmit, Phase::TranslateCompact, Phase::TranslateBatch,
   Phase::FramePrepare, Phase::FrameAlloc, Phase::FrameCopy, Phase::FrameMaintain,
   Phase::FrameBuild, Phase::FrameRegions, Phase::FrameMap, Phase::FrameRelocate,
   Phase::Native, Phase::Doorbell, Phase::Publication, Phase::CommandUpload,
   Phase::DescriptorUpload, Phase::BarFlush, Phase::GpPut];
 let mut at=10;
 for p in phases { s.switch(p,at);at+=10; }
 // Restoring nested scopes must not bill their elapsed time twice.
 s.switch(Phase::Doorbell,at);at+=10;
 s.switch(Phase::Native,at);at+=10;
 s.switch(Phase::Cleanup,at);at+=10;
 let (total,ns)=s.finish(at);
 assert_eq!(total,300);
 assert_eq!(ns.iter().sum::<u64>(),total);
 for p in phases { assert!(ns[p as usize]>=10,"missing {p:?}"); }
 assert_eq!(ns[Phase::Doorbell as usize],20);
 assert_eq!(ns[Phase::Native as usize],20);
 assert_eq!(ns.len(),PHASES.len());
 let early=Split::new(0);early.switch(Phase::CommandUpload,5);
 assert_eq!(early.finish(13).1.iter().sum::<u64>(),13);
 let f=Facts::new();
 f.bytes(cost::Bytes::Command,512);f.bytes(cost::Bytes::Command,256);
 f.bytes(cost::Bytes::Descriptor,8);f.bytes(cost::Bytes::Receipt,16);
 f.bytes(cost::Bytes::Frame,1024);f.bytes(cost::Bytes::FrameZero,4096);
 f.bytes(cost::Bytes::GpPut,4);
 // Native sequences 5 and 9 reuse physical slot 0. Time is inclusive wait
 // latency, reported separately from the exclusive submit partition.
 f.wait_on(Some((0,101,5)),100);
 f.wait_on(Some((0,101,5)),110);
 f.wait_on(Some((0,102,9)),140);
 f.wait_on(None,170);
 let (bytes,waits)=f.finish(200);
 assert_eq!(bytes,[768,8,16,1024,4096,4]);
 assert_eq!(waits[0],[2,70,101,102,5,9]);
 assert!(waits[1..].iter().flatten().all(|v|*v==0));
 let mut totals=cost::Totals::new();
 assert!(totals.record(3,total,&ns));
 assert!(totals.record(3,total,&ns));
 let rows=totals.drain();assert_eq!(rows[0][1],2);
 assert_eq!(rows[0][3..].iter().sum::<u64>(),2*total);
 let mut r=timing::Recorder::new();
 r.submit(3,total,&ns);r.facts(3,bytes,waits);
 r.submit(3,total,&ns);r.facts(3,bytes,waits);
 println!("{}",r.drain(1).unwrap());assert!(r.drain(2).is_none());
}
'''.replace('MODULE', str(ROOT / 'kernel/src/gpu/gk208/timing.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            result = subprocess.run([str(p / 'check')], check=True, capture_output=True, text=True)
            row = json.loads(result.stdout.removeprefix('GT710_TIMING_V1 '))
            self.assertEqual(row['dropped'], 0)
            self.assertEqual(sum(row['submits'][0][3:]), row['submits'][0][2])
            self.assertEqual(row['submit_bytes'], [[3, 1536, 16, 32, 2048, 8192, 8]])
            self.assertEqual(row['admission_waits'], [[3, 0, 4, 140, 101, 102, 5, 9]])
            self.assertEqual(row['submit_mappings']['command'], 'coherent WB system memory GPU DMA')
            self.assertEqual(row['submit_mappings']['gp_put'], 'write-combined BAR1 USERD')


if __name__ == '__main__':
    unittest.main()
