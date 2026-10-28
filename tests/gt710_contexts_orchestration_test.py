"""Exercise the four-frame producer with device IO substituted, without a boot."""
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item
from gt710_contexts_report_test import triangle, V1, V2


class ContextsOrchestration(unittest.TestCase):
    def test_alternation_and_state_free_restore(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/mod.rs').read_text()
        flow = item(source, 'fn contexts_sequence(')
        evidence = source[source.index('#[derive(Default)]\nstruct ContextsEvidence'):source.index('struct ShaderUpload')]
        for old, new in [('super::bulk', 'bulk'), ('super::hash', 'hash')]:
            evidence = evidence.replace(old, new)
        fixture = (ROOT / 'tests/fixtures/gt710_contexts_orchestration.rs').read_text()
        fixture = fixture.replace('ROOT', str(ROOT)).replace('// FLOW', flow).replace('// EVIDENCE', evidence)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for variant, data in [('v1', triangle(V1)), ('v2', triangle(V2))]:
                (path / (variant + '.bin')).write_bytes(data)
            (path / 'contexts.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(path / 'contexts.rs'), '-o', str(path / 'contexts')], check=True)
            result = subprocess.run([str(path / 'contexts'), '--nocapture'], check=True, capture_output=True, text=True)
            print('\n'.join(line for line in result.stdout.splitlines() if not line.startswith('GT710_CTX_IMAGE ')))
        report = json.loads(next(line.split(' ', 1)[1] for line in result.stdout.splitlines() if line.startswith('GT710_CONTEXTS ')))
        self.assertIsNone(report['error'])
        frames = report['frames']
        self.assertEqual([f['case'] for f in frames], ['a-v1', 'b-gradient', 'a-v2', 'b-restored'])
        self.assertEqual(len({(f['receipt']['epoch'], f['receipt']['nonce']) for f in frames}), 4)
        self.assertTrue(all(f['accepted'] for f in frames))
        self.assertEqual(len(report['readbacks']), 8)
        rows = [line+'\n' for line in result.stdout.splitlines() if line.startswith('GT710_CTX_IMAGE ')]
        anchor = next(line.split() for line in result.stdout.splitlines() if line.startswith('GT710_CTX_BULK '))
        body = ''.join(rows).encode()
        self.assertEqual([str(len(body)), hashlib.sha256(body).hexdigest()], anchor[1:])
        self.assertEqual([row.split()[1:3] for row in rows[::64]],
                         [[name, str(p)] for name in ['a-v1', 'b-gradient', 'a-v2', 'b-restored'] for p in (85, 170)])
        for record in report['readbacks']:
            image = bytearray()
            for row in rows:
                fields = row.split()
                if fields[1:3] == [record['case'], str(record['poison'])]:
                    for run in fields[4].split(','):
                        count, word = run.split(':')
                        image.extend(int(word, 16).to_bytes(4, 'little') * int(count, 16))
            self.assertEqual(len(image), 16384)
            self.assertEqual(hashlib.sha256(image).hexdigest(), record['sha256'])
        draw = (ROOT / 'kernel/src/gpu/gk208/gr/draw.rs').read_text()
        for name, oracle in [('V2_VERTEX_WORDS', V2)]:
            words = draw.split('pub const '+name)[1].split('= [', 1)[1].split('];', 1)[0]
            parsed = [int(w.strip(), 16) for w in words.split(',') if w.strip()]
            expected = [struct.unpack('<I', struct.pack('<f', float(v)))[0]
                        for vertex in oracle['vertices'] for v in vertex['clip'] + vertex['rgba']]
            self.assertEqual(parsed, expected)
        # Ownership returns to the pair even when the producer fails.
        probe = item(source, 'fn contexts_probe(')
        self.assertLess(probe.index('contexts_sequence('), probe.index('graphics.second = Some(second)'))
        self.assertIn('contexts_probe(', item(source, 'fn probe_inner('))


if __name__ == '__main__':
    unittest.main()
