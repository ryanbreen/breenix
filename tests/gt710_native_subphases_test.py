"""Partition native CPU preparation without overlapping its publication cost."""
from pathlib import Path
import json
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class NativeSubphases(unittest.TestCase):
    def test_native_phase_export_and_strict_partition(self):
        source = r'''
extern crate alloc;
#[path="TIMING"]pub mod timing;
use timing::submit_cost::{Split,Phase};
fn main(){
 let split=Split::new(0);split.switch(Phase::Native,1);
 for (n,p)in[Phase::NativeBind,Phase::NativePayload,Phase::NativeBuild,Phase::NativeCache].into_iter().enumerate(){split.switch(p,2+n as u64*10);}
 split.switch(Phase::Native,42);split.switch(Phase::Doorbell,43);
 let(total,phases)=split.finish(50);assert_eq!(total,50);assert_eq!(phases.iter().sum::<u64>(),50);
 assert_eq!(phases[Phase::Native as usize],2);assert_eq!(phases[Phase::Doorbell as usize],7);
 for p in[Phase::NativeBind,Phase::NativePayload,Phase::NativeBuild,Phase::NativeCache]{assert_eq!(phases[p as usize],10);}
 let mut r=timing::Recorder::new();r.submit(3,total,&phases);println!("{}",r.drain(1).unwrap());
}
'''.replace('TIMING', str(ROOT / 'kernel/src/gpu/gk208/timing.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-native-subphases-') as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            result = subprocess.check_output([str(p / 'check')], text=True)
        data = json.loads(result.split(' ', 1)[1])
        self.assertEqual(data['submit_phases'][-4:], ['native_bind', 'native_payload', 'native_build', 'native_cache'])
        validate = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))['validate_submit_costs']
        validate(data)
        bad = dict(data, submits=[data['submits'][0].copy()])
        bad['submits'][0][-1] += 1
        with self.assertRaises(ValueError):
            validate(bad)
        with self.assertRaises(ValueError):
            validate(dict(data, submit_phases=data['submit_phases'][:-1]))


if __name__ == '__main__':
    unittest.main()
