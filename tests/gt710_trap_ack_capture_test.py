"""Exercise the failing draw record and production FIFO cleanup with W1C MMIO.

The register model holds FECS preempt pending until GR status is acknowledged
and the FIFO is resumed. It supplies no rendered pixels or hardware verdict.
"""
from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class TrapAckCapture(unittest.TestCase):
    def test_failed_draw_capture_ack_and_unbind(self):
        gr = (ROOT / 'kernel/src/gpu/gk208/gr/mod.rs').read_text()
        channel = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        flow = gr[gr.index('            let mut draw_evidence ='):gr.index('            draw_result?;')]
        evidence = gr[gr.index('#[derive(Default)]\nstruct DrawEvidence'):gr.index('struct ShaderUpload')]
        evidence = evidence.replace('super::bulk', 'bulk').replace('super::hex', 'hex').replace('super::hash', 'hash')
        methods = '\n'.join(item(channel, m) for m in [
            '        fn sequence(', '        pub(in crate::gk208::gr) fn draw(',
            '        pub(in crate::gk208::gr) fn quiesce('])
        methods = methods.replace('pub(in crate::gk208::gr)', 'pub(crate)').replace('super::super::', 'crate::')
        harness = (ROOT / 'tests/fixtures/gt710_trap_ack_capture.rs').read_text()
        # Use the production MMIO adapter when it exists; the first red tree
        # has neither the adapter nor a capture/ack call.
        adapter = item(gr, 'impl binding::TrapIo for Registers') if 'impl binding::TrapIo for Registers' in gr else ''
        for key, value in [('ROOT', str(ROOT)), ('METHODS', methods), ('EVIDENCE', evidence), ('FLOW', flow), ('ADAPTER', adapter)]:
            harness = harness.replace('// ' + key if key != 'ROOT' else key, value)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'trap.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path / 'trap.rs'), '-o', str(path / 'trap')], check=True)
            result = subprocess.run([str(path / 'trap'), '--nocapture', '--test-threads=1'], capture_output=True, text=True)
            print(result.stdout)
            print(result.stderr)
            reports = [json.loads(line.split('GT710_DRAW ', 1)[1]) for line in result.stdout.splitlines() if 'GT710_DRAW ' in line]
            self.assertEqual(len(reports), 4)
            for report in reports:
                self.assertIsNotNone(report['error'], 'acknowledging a trap must not accept the draw')
                capture = report.get('trap_capture')
                self.assertIsNotNone(capture, 'missing pre-ack GR method/error capture')
                self.assertEqual(capture['registers'], [0x400704, 0x400708, 0x40070c, 0x400500, 0x409b00, 0x404208])
                self.assertEqual(capture['values'], [0x21400, 0xdeadbeef, 0x1234, 0, 0x80123456, 0xa197])
                self.assertEqual(capture['status'][2], 4)
            self.assertEqual(result.returncode, 0, 'production teardown failed its MMIO ordering assertions')


if __name__ == '__main__':
    unittest.main()
