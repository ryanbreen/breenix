"""Exclusive submit phases must account for every call, including ordinary icon draws."""
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SubmitAccounting(unittest.TestCase):
    def test_nested_publication_and_cleanup_partition_elapsed_time(self):
        source = f'''extern crate alloc;
#[path="{ROOT / 'kernel/src/gpu/gk208/submit_cost.rs'}"] pub mod submit_cost;
use submit_cost::{{Split, Phase, PHASES}};
fn main() {{
 let s = Split::new(100);
 assert_eq!(s.switch(Phase::Copy, 110), Phase::Entry);
 s.switch(Phase::Validate, 130);
 s.switch(Phase::Admission, 150);
 s.switch(Phase::Setup, 180);
 s.switch(Phase::Translate, 200);
 s.switch(Phase::FramePrepare, 250);
 s.switch(Phase::Fence, 270);
 let previous = s.switch(Phase::Native, 300);
 let native = s.switch(Phase::Doorbell, 340);
 s.switch(native, 370);
 s.switch(previous, 380);
 s.switch(Phase::Output, 390);
 s.switch(Phase::Cleanup, 400);
 let (total, phases) = s.finish(430);
 assert_eq!(total,330);
 assert_eq!(phases, [10,20,20,30,20,50,20,40,50,30,10,30]);
 assert_eq!(phases.iter().sum::<u64>(),total);
 // Refusals/early returns are partitioned too; no call to begin_draw is needed.
 let early=Split::new(500);early.switch(Phase::Copy,510);
 let (total,phases)=early.finish(520);assert_eq!(total,20);
 assert_eq!(phases.iter().sum::<u64>(),total);
 assert_eq!(PHASES.len(),phases.len());
 let mut totals=submit_cost::Totals::new();
 for _ in 0..10154 {{totals.record(3,330,&s.finish(430).1);}}
 totals.record(1,20,&phases);
 let rows=totals.drain();assert_eq!(rows.len(),2);
 let icon=rows.iter().find(|r|r[0]==3).unwrap();
 assert_eq!(&icon[..3],&[3,10154,3350820]);
 assert_eq!(icon[3..].iter().sum::<u64>(),icon[2]);
 assert!(totals.drain().is_empty());
}}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)

    def test_scorer_refuses_missing_or_double_counted_phases(self):
        api = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
        validate = api['validate_submit_costs']
        names = ['entry', 'copy_in', 'validation', 'admission_poll', 'setup',
                 'translate', 'frame_prepare', 'fence_bookkeeping', 'native_submit',
                 'doorbell', 'copy_out', 'cleanup']
        good = dict(submit_phases=names, submits=[[3, 10154, 120] + [10] * 12])
        validate(good)
        validate({})  # Historical captures are readable, never a new measurement.
        for bad in [dict(good, submits=[[3, 10154, 119] + [10] * 12]),
                    dict(good, submit_phases=names[:-1]),
                    dict(submits=good['submits']), dict(submit_phases=names),
                    dict(good, submits=[[3, 0, 120] + [10] * 12]),
                    dict(good, submits=good['submits'] * 2)]:
            with self.assertRaises(ValueError):
                validate(bad)


if __name__ == '__main__':
    unittest.main()
