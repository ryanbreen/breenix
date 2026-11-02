"""Count duplicate status IO around the production adapter's receipt sampler."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReapPollCost(unittest.TestCase):
    def test_receipt_health_is_the_single_status_scan(self):
        source = (ROOT / 'tests/gt710_adapter_test.rs').read_text()
        source = source.replace('../../kernel/', str(ROOT / 'kernel') + '/')
        source = source.replace('../kernel/', str(ROOT / 'kernel') + '/')
        # Count the production MMU decoder's IO, independently of the existing
        # channel fixture's health/sample counters. Do not replace the adapter.
        source = source.replace('fn read(&mut self,address:usize)->u32 {',
                                'fn read(&mut self,address:usize)->u32 { '
                                'STATUS_READS.with(|n| n.set(n.get()+1));')
        source += r'''
thread_local! { static STATUS_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[test]
fn healthy_receipt_does_not_precede_its_sampler_with_another_trap_scan() {
    for ready in [false, true] {
        let (mut d, s) = device(); send(&mut d, 9001);
        { let mut s = s.borrow_mut(); s.health_calls = 0; s.sample_calls = 0;
          if ready { s.release = 1; } }
        STATUS_READS.with(|n| n.set(0));
        let result = d.reap();
        assert_eq!(s.borrow().health_calls, 1, "receipt still checks hardware health");
        assert_eq!(s.borrow().sample_calls, 1);
        assert_eq!(STATUS_READS.with(std::cell::Cell::get), 0,
                   "an independent trap scan duplicates the sampler's health reads");
        if ready { assert!(matches!(result, Reap::Done(_))); }
        else { assert!(matches!(result, Reap::Empty)); assert_eq!(d.outstanding(), 1); }
    }
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-reap-cost-') as directory:
            p = Path(directory)
            (p / 'adapter.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                           '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                           '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}',
                           str(p / 'adapter.rs'), '-o', str(p / 'adapter')], check=True)
            arguments = [] if '--all-adapter-cases' in sys.argv else ['healthy_receipt_does_not_precede']
            subprocess.run([str(p / 'adapter'), *arguments, '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main(argv=[x for x in sys.argv if x != '--all-adapter-cases'])
