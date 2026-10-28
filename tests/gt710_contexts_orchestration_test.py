"""Exercise the eight-frame producer with device IO substituted, without a boot."""
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
from gt710_contexts_report_test import triangle, V1, V2, STEPS, API, fixture as scorer_fixture, capture


class ContextsOrchestration(unittest.TestCase):
    def test_native_release_and_binding_order(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        methods = '\n'.join(item(source, marker) for marker in [
            '        fn sequence(', '        pub(in crate::gk208::gr) fn context_draw('])
        methods = methods.replace('pub(in crate::gk208::gr)', 'pub(crate)').replace('super::super::', 'crate::')
        fixture = (ROOT / 'tests/fixtures/gt710_contexts_submission.rs').read_text()
        fixture = fixture.replace('ROOT', str(ROOT)).replace('// METHODS', methods)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'submission.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(path / 'submission.rs'), '-o', str(path / 'submission')], check=True)
            subprocess.run([str(path / 'submission'), '--nocapture'], check=True)

    def test_alternation_and_state_free_restore(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/mod.rs').read_text()
        flow = item(source, 'fn contexts_sequence(').replace('super::hash', 'hash')
        evidence = source[source.index('#[derive(Default)]\nstruct ContextsEvidence'):source.index('#[derive(Default)]\nstruct DrawEvidence')]
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
            result = subprocess.run([str(path / 'contexts'), 'context_', '--nocapture'], capture_output=True, text=True)
            if result.returncode:
                print(result.stdout, result.stderr)
            result.check_returncode()
            print('\n'.join(line for line in result.stdout.splitlines() if not line.startswith('GT710_CTX_IMAGE ')))
        report = json.loads(next(line.split(' ', 1)[1] for line in result.stdout.splitlines() if line.startswith('GT710_CONTEXTS ')))
        self.assertIsNone(report['error'])
        frames = report['frames']
        self.assertEqual([f['case'] for f in frames], [name for name, _, _ in STEPS])
        self.assertEqual(len({(f['receipt']['epoch'], f['receipt']['nonce']) for f in frames}), 8)
        self.assertEqual([f['accepted'] for f in frames], [True]*4 + [False, True, False, True])
        self.assertEqual(len(report['readbacks']), 16)
        rows = [line+'\n' for line in result.stdout.splitlines() if line.startswith('GT710_CTX_IMAGE ')]
        anchor = next(line.split() for line in result.stdout.splitlines() if line.startswith('GT710_CTX_BULK '))
        body = ''.join(rows).encode()
        self.assertEqual([str(len(body)), hashlib.sha256(body).hexdigest()], anchor[1:])
        self.assertEqual([row.split()[1:3] for row in rows[::64]],
                         [[name, str(p)] for name, _, _ in STEPS for p in (85, 170)])
        images = {}
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
            images[record['case'], record['poison']] = bytes(image)
        # Feed production metadata and retained synthetic device images to u3's scorer.
        channels, _ = scorer_fixture()
        report['channels'] = channels['channels']
        for i, channel in enumerate(report['channels']):
            channel.update(instance=0x600000+i*4096, image=0x500000+i*4096,
                           target=0x100000+i*0x100000, release=0x900000+i*4096, epoch=42+i)
        serial, encoded = capture(report, images)
        self.assertEqual([r['result'] for r in API['cases']('gr-contexts', encoded+serial)], ['PASS']*2)
        for control, successor in [('suppress', 'after-suppress'), ('corrupt', 'after-corrupt')]:
            rejected = json.loads(json.dumps(report))
            rejected['frames'][next(i for i, f in enumerate(frames) if f['case'] == control)]['accepted'] = True
            serial, encoded = capture(rejected, images)
            self.assertEqual(API['cases']('gr-contexts', encoded+serial)[1]['result'], 'FAIL')
            # A control image presented as its clean successor must fail the pixel oracle.
            bad = dict(images)
            for poison in (85, 170): bad[successor, poison] = images[control, poison]
            serial, encoded = capture(report, bad)
            self.assertEqual(API['cases']('gr-contexts', encoded+serial)[1]['result'], 'FAIL')
        offset = (report['channels'][1]['epoch'] % 4096)*4
        self.assertTrue(0 <= offset <= 16380)
        expected = bytearray(images['b-restored', 85]); expected[offset:offset+4] = bytes(4)
        self.assertEqual(images['corrupt', 85], expected)
        self.assertEqual(images['suppress', 85], bytes(16384))
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
        self.assertLess(probe.index('session.execute(hardware)?'), probe.index('session.bind_shaders('))
        self.assertIn('contexts_probe(', item(source, 'fn probe_inner('))


if __name__ == '__main__':
    unittest.main()
