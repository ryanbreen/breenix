#!/usr/bin/env python3
"""Score QEMU UI evidence receipts against their serial anchors."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SEQUENCE = runpy.run_path(str(ROOT / 'scripts/suite-sequence.py'))


def digest(body):
    return hashlib.sha256(body).hexdigest()


class UploadVerdictTest(unittest.TestCase):
    def test_receipts_and_serial_anchors(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / 'repo'
            (repo / 'scripts').mkdir(parents=True)
            (repo / 'scripts/suite-verdict.py').symlink_to(ROOT / 'scripts/suite-verdict.py')
            (repo / 'docs/suites').mkdir(parents=True)
            (repo / 'docs/suites/ui-testing.json').write_text(json.dumps({
                'id': 'ui-testing', 'categories': [{'id': 'proof', 'cases': [{'id': 'one'}]}]}))
            serial = root / 'serial.txt'
            uploads = Path(str(serial) + '.evidence')
            uploads.mkdir()
            token, boot = 'a' * 64, 'boot-id'
            route = '/gt710-upload/' + token
            (uploads / 'route.json').write_text(json.dumps({'port': 1234, 'token': token}))
            (uploads / 'gt710-upload.json').write_text(json.dumps({'id': boot, 'route': route, 'suite': 'ui-testing'}))
            label = 'uitest-' + digest(b'proof/one')[:16]
            bodies = [('gt710-pixels.rle', 'native-upload.json', b'start', ''),
                      ('gt710-final.rle', 'native-final-upload.json', b'final', '/final'),
                      (f'diagnostic-{label}.bin', f'native-diagnostic-{label}.json', b'bundle', '/diagnostics/' + label)]
            for filename, receipt, body, suffix in bodies:
                (uploads / filename).write_bytes(body)
                (uploads / receipt).write_text(json.dumps(dict(id=boot, route=route + suffix, complete=True,
                                                               bytes=len(body), sha256=digest(body))))
            text = (f'SUITE_EVIDENCE ui-testing startup bytes=5 sha256={digest(b"start")}\n'
                    'SUITE ui-testing START cases=1\n'
                    'SUITE ui-testing MEMORY before free=100 heap_free_kb=1 other=0\n'
                    f'SUITE ui-testing EVIDENCE bundle proof/one label={label} bytes=6 sha256={digest(b"bundle")} result=ok path=/var/uitest/ui-testing/proof/one\n'
                    'SUITE ui-testing CASE proof/one PASS ms=1\n'
                    'SUITE ui-testing DONE passed=1 failed=0 skipped=0 total=1\n'
                    'SUITE ui-testing MEMORY after free=100 heap_free_kb=1 other=0\n'
                    'SUITE ui-testing MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                    f'SUITE_EVIDENCE ui-testing suite-final-panel bytes=5 sha256={digest(b"final")}\n')
            serial.write_text(text)
            def score():
                return SEQUENCE['score'](['ui-testing'], serial, [serial], repo=repo)
            self.assertTrue(score()[0], score()[1])
            receipt = uploads / f'native-diagnostic-{label}.json'
            original = receipt.read_text()
            receipt.unlink()
            self.assertFalse(score()[0], 'missing bundle receipt passed')
            receipt.write_text(original)
            receipt.write_text(original.replace(digest(b'bundle'), 'b' * 64))
            self.assertFalse(score()[0], 'wrong bundle digest passed')
            receipt.write_text(original)
            serial.write_text(text.replace(text.splitlines()[-1] + '\n', ''))
            self.assertFalse(score()[0], 'missing final panel passed')
            serial.write_text(text.replace('result=ok', 'result=disk-only:no-route'))
            self.assertFalse(score()[0], 'staged route accepted disk-only bundle')
            serial.write_text(text.replace(text.splitlines()[-1], 'SUITE_EVIDENCE ui-testing suite-final-panel skipped=no-route'))
            self.assertFalse(score()[0], 'staged route accepted skipped panel')


if __name__ == '__main__':
    unittest.main()
