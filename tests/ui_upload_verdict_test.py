#!/usr/bin/env python3
"""Score QEMU UI evidence receipts against their serial anchors."""
import hashlib
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

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
            # Exercise the actual standalone launcher's scoring branch, keeping
            # only process management and the unrelated wall clock out of scope.
            watcher = runpy.run_path(str(ROOT / 'scripts/watch-qemu.py'))
            main = watcher['main']
            def standalone():
                clock = Mock()
                clock.verdict.return_value = (True, 'clock fixture')
                with patch.dict(main.__globals__, ROOT=repo, watch=lambda *a: 'done',
                                stop=lambda *a: None, WallClockCheck=lambda *a: clock), \
                     patch('subprocess.Popen'), \
                     patch.object(sys, 'argv', ['watch-qemu', '--serial', str(serial),
                                               '--mode', 'suite', '--suite', 'ui-testing', '--', 'fixture']):
                    return main()
            self.assertEqual(standalone(), 0)
            for bad_text in (text.replace('result=ok', 'result=disk-only:no-route'),
                             text.replace(text.splitlines()[-1], 'SUITE_EVIDENCE ui-testing suite-final-panel skipped=no-route')):
                serial.write_text(bad_text)
                self.assertNotEqual(standalone(), 0, 'real standalone scoring accepted failed upload')
            serial.write_text(text)
            payload = uploads / 'gt710-final.rle'
            payload.write_bytes(b'wrong')
            self.assertNotEqual(standalone(), 0, 'real standalone scoring accepted corrupt upload')
            payload.write_bytes(b'final')
            route_info = (uploads / 'route.json').read_text()
            (uploads / 'route.json').unlink()
            self.assertNotEqual(standalone(), 0, 'missing receiver route bypassed validation')
            (uploads / 'route.json').write_text(route_info)
            ok, rows = score()
            self.assertTrue(ok, rows)
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
            panel = b'x' * ((2 << 20) + 1)
            parts = f'SUITE_PARTS v=1 first=0 parts=2 bytes={len(panel)} sha256={digest(panel)}\n'.encode()
            (uploads / 'gt710-pixels.rle').write_bytes(parts)
            (uploads / 'native-upload.json').write_text(json.dumps(dict(id=boot, route=route, complete=True,
                                                                        bytes=len(parts), sha256=digest(parts))))
            for index, chunk in enumerate((panel[:2 << 20], panel[2 << 20:])):
                (uploads / f'gt710-breengel-{index:04}.rle').write_bytes(chunk)
                (uploads / f'native-picture-upload-{index:04}.json').write_text(json.dumps(dict(
                    id=boot, route=route + f'/pictures/{index}', complete=True, bytes=len(chunk), sha256=digest(chunk))))
            serial.write_text(text.replace(f'bytes=5 sha256={digest(b"start")}',
                                           f'bytes={len(panel)} sha256={digest(panel)}'))
            self.assertTrue(score()[0], 'matching multipart panel failed')
            (uploads / 'gt710-breengel-0001.rle').write_bytes(b'z')
            self.assertFalse(score()[0], 'corrupt multipart panel passed')


if __name__ == '__main__':
    unittest.main()
