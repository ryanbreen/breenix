"""Exercise shared SUBMIT refusals and native reservation/encoding on the host.

Extract the production validation/errno block and native reservation, and use
the real ABI validator, quota counter and GK208 encoder. Only the snapshotted
resource view and the final ring publication are modeled; no GPU is claimed.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class StreamOverflow(unittest.TestCase):
    def test_native_boundaries_and_whole_stream_refusals(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        submit = item(core, 'fn submit_stream(')
        validation = submit[submit.index('    let plan = match stream::validate('):
                            submit.index('    if let Some(extra) = extra.as_mut()')]
        reservation = submit[submit.index('                let bytes = crate::gk208::encode::'):
                             submit.index('                // Native scratch uses')]
        # Refusals must precede slot admission, native preparation and publication.
        self.assertLess(submit.index('let plan = match stream::validate('), submit.index('let admitted ='))
        self.assertLess(submit.index('let plan = match stream::validate('), submit.index(reservation))
        self.assertLess(submit.index(reservation), submit.index('::translate(&words'))
        quota = (ROOT / 'kernel/src/gpu/quota.rs').read_text()
        quota = '\n'.join(item(quota, marker) for marker in (
            'pub(super) struct Counts {', 'impl Counts {',
            'pub(super) struct QuotaCharge {', 'impl QuotaCharge {',
            'impl Drop for QuotaCharge {'))
        quota = quota.replace('pub(super)', 'pub')
        quota = '#[derive(Default)]\n' + quota
        fixture = (ROOT / 'tests/fixtures/gt710_stream_overflow.rs').read_text()
        fixture = fixture.replace('// VALIDATION', validation).replace('// RESERVATION', reservation)
        fixture = fixture.replace('// QUOTA', quota).replace('ROOT/', str(ROOT) + '/')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            abi = directory / 'libgpu_abi.rlib'
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                '--crate-type=lib', '--crate-name=gpu_abi', '--cfg', 'feature="alloc"',
                str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(abi)], check=True)
            path = directory / 'overflow.rs'
            path.write_text(fixture)
            binary = directory / 'overflow'
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={abi}', str(path), '-o', str(binary)], check=True)
            subprocess.run([str(binary), '--test-threads=1', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
