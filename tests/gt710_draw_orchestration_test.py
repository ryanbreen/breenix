"""Run the draw producer and cleanup slices with injected device boundaries.

The host checks ownership/order and emitted bytes; it does not emulate shaders
or claim GPU execution. The metal scorer remains the colour oracle.
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


class DrawOrchestration(unittest.TestCase):
    def test_producer_order_failures_and_bulk(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/mod.rs').read_text()
        # Missing production orchestration is the expected first red result.
        flow = item(source, 'fn draw_probe(')
        evidence = source[source.index('#[derive(Default)]\nstruct DrawEvidence'):source.index('struct ShaderUpload')]
        finish = item(source, '    fn finish(mut self)').replace('super::channel::VmIo', 'VmIo')
        for old, new in [('super::bulk', 'bulk'), ('super::hash', 'hash'), ('super::hex', 'hex')]:
            evidence = evidence.replace(old, new)
        fixture = (ROOT / 'tests/fixtures/gt710_draw_orchestration.rs').read_text()
        fixture = fixture.replace('// TRAP_CAPTURE', item((ROOT / 'kernel/src/gpu/gk208/gr/binding.rs').read_text(), 'pub struct TrapCapture'))
        fixture = fixture.replace('// FLOW', flow).replace('// EVIDENCE', evidence).replace('// FINISH', finish)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'draw.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'draw.rs'), '-o', str(path / 'draw')], check=True, cwd=ROOT)
            result = subprocess.run([str(path / 'draw'), '--nocapture'], check=True, capture_output=True, text=True)
            print(result.stdout)
            rows = [line for line in result.stdout.splitlines() if line.startswith('GT710_DRAW_IMAGE ')]
            self.assertEqual([row.split()[1:3] for row in rows[::64]],
                             [[name, str(poison)] for name in ['triangle', 'vertex-control', 'after-vertex', 'fragment-control', 'after-fragment'] for poison in [85, 170]])
            report = json.loads(next(line[11:] for line in result.stdout.splitlines() if line.startswith('GT710_DRAW ')))
            self.assertIsNone(report['error'])
            self.assertEqual(len(report['releases']), 5)
            self.assertEqual(len({(r['epoch'], r['nonce']) for r in report['releases']}), 5)
            self.assertEqual(report['trap_registers'], [0x400100, 0x400108, 0x400110, 0x400118])
            self.assertEqual(report['trap_before'], [0]*4)
            self.assertEqual(report['trap_after'], [0]*4)
            self.assertEqual(len(report['readbacks']), 10)
            # Include measured prior boot bulk plus a conservative allowance for
            # presentation rows (the later unit supplies those).
            self.assertLess(865396 + sum(len(row)+1 for row in rows) + 128*1024, 2*1024*1024)
            for r in report['readbacks']:
                data = bytes.fromhex(''.join(row.split()[4] for row in rows if row.split()[1:3] == [r['case'], str(r['poison'])]))
                self.assertEqual(len(data), 16384)
                self.assertEqual(r['bytes'], len(data))
        channel = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        self.assertNotIn('self.quiesce(', item(channel, '        pub(in crate::gk208::gr) fn bind_shaders('))
        # The production caller must return the same session to cleanup before
        # propagating errors from this unit.
        caller = source[source.index('let mut draw_evidence ='):source.index('interrupt_snapshot(hardware.regs, hardware.device, "channel-drained")')]
        self.assertLess(caller.index('draw_probe('), caller.index('hardware.graphics = Some(graphics)'))
        self.assertLess(caller.index('hardware.graphics = Some(graphics)'), caller.index('draw_result?'))


if __name__ == '__main__':
    unittest.main()
