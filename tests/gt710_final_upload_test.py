"""G12: independent one-use slots and authenticated late evidence reconstruction."""
import hashlib
import io
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import gt710_upload as upload
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))


class Handler:
    def __init__(self, route, body, sha=None):
        self.path = route
        self.rfile = io.BytesIO(body)
        self.connection = self
        self.headers = {'Content-Length': str(len(body)),
                        'X-Content-SHA256': sha or hashlib.sha256(body).hexdigest()}
        self.code = None
    def settimeout(self, value): pass
    def send_error(self, code, *args): self.code = code
    def send_response(self, code): self.code = code
    def send_header(self, *args): pass
    def end_headers(self): pass


def anchor(prefix, body):
    return f'{prefix} {len(body)} {hashlib.sha256(body).hexdigest()}\n'


class FinalUpload(unittest.TestCase):
    def test_slots_are_independent_once_ordered_and_bound_to_the_powered_lease(self):
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            boot = state / 'boots/sample'
            boot.mkdir(parents=True)
            with patch.object(upload.m, 'STATE', state), patch.object(upload.m, 'read_lease') as lease:
                lease.return_value = dict(id='sample', state='powered')
                route = upload.prepare(boot, 'sample', {'BOOT_URL': 'http://fixture.invalid:8080/boot'})
                from urllib.parse import urlsplit
                route = urlsplit(route).path
                def send(path, body=b'rows\n', sha=None):
                    h = Handler(path, body, sha); upload.receive(h); return h.code
                self.assertEqual(send(route + '/final'), 404)
                self.assertEqual(send(route, b'startup\n'), 200)
                self.assertEqual(send(route), 404)
                self.assertEqual(send(route + '/final', sha='0'*64), 400)
                self.assertFalse((boot / 'gt710-final.rle').exists())
                for invalid in [dict(id='other', state='powered'), dict(id='sample', state='released')]:
                    lease.return_value = invalid
                    self.assertEqual(send(route + '/final'), 404)
                lease.return_value = dict(id='sample', state='powered')
                (boot / 'ended').touch()
                self.assertEqual(send(route + '/final'), 400)
                (boot / 'ended').unlink()
                self.assertEqual(send(route + '/final', b'late\n'), 200)
                self.assertEqual(send(route + '/final'), 404)
                self.assertEqual((boot / 'gt710-pixels.rle').read_bytes(), b'startup\n')
                self.assertEqual((boot / 'gt710-final.rle').read_bytes(), b'late\n')
                self.assertTrue(upload.m.read_json(boot / 'native-upload.json')['complete'])
                self.assertTrue(upload.m.read_json(boot / 'native-final-upload.json')['complete'])

    def test_union_requires_both_digests_and_exact_serial_anchors(self):
        first = b'{"phase":"caps-probe","capability":3,"first":"1122","second":"1122"}\n'
        late = b'{"phase":"caps-reprobe","capability":3,"epoch":2,"first":"1122","second":"1122"}\n'
        probe = lambda prefix, row: prefix + ' id=3' + (' epoch=2' if 'REPROBE' in prefix else '') + ' sha256=' + hashlib.sha256(row.rstrip(b'\n')).hexdigest() + '\n'
        startup = probe('GT710_CAPS_PROBE', first)
        runtime = probe('GT710_CAPS_REPROBE', late)
        serial = (startup + anchor('GT710_BULK', first) + 'SUITE gpu START cases=170\n'
                  + runtime + 'SUITE gpu CASE performance/present-rate-2560 FAIL ms=1\n'
                  + 'SUITE gpu DONE passed=106 failed=64 skipped=0 total=170\n'
                  + 'SUITE gpu MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n'
                  + anchor('GT710_FINAL_BULK', late) + 'SUITE_SEQUENCE READY gpu\n')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            def stage(text=serial, body=late):
                (path / 'serial.txt').write_text(text)
                (path / 'gt710-pixels.rle').write_bytes(first)
                (path / 'gt710-final.rle').write_bytes(body)
            stage()
            result = API['capture_serial'](path)
            self.assertIn(first.decode() + startup, result)
            self.assertIn(late.decode() + runtime, result)
            mutations = [
                (serial, late.replace(b'1122', b'1123')),
                (serial.replace(runtime, ''), late),
                (serial.replace(anchor('GT710_FINAL_BULK', late), ''), late),
                (serial.replace(anchor('GT710_FINAL_BULK', late), anchor('GT710_FINAL_BULK', late)*2), late),
                (serial.replace(anchor('GT710_FINAL_BULK', late), '').replace('SUITE gpu START', anchor('GT710_FINAL_BULK', late)+'SUITE gpu START'), late),
                (serial.replace('SUITE gpu DONE passed=106 failed=64 skipped=0 total=170\n', ''), late),
                (serial.replace('SUITE_SEQUENCE READY gpu\n', ''), late),
                (serial.replace(runtime, runtime*2), late),
                (serial.replace(anchor('GT710_FINAL_BULK', late), anchor('GT710_FINAL_BULK', first+late)), first+late),
            ]
            for text, body in mutations:
                with self.subTest(text=text, body=body):
                    stage(text, body)
                    with self.assertRaises(ValueError): API['capture_serial'](path)
            stage(); (path / 'gt710-final.rle').unlink()
            with self.assertRaises((ValueError, OSError)): API['capture_serial'](path)

    def test_console_owns_the_legacy_ce_producers(self):
        console = (ROOT / 'userspace/programs/src/suite_console.rs').read_text()
        gpu = (ROOT / 'userspace/programs/src/suite_gpu.rs').read_text()
        self.assertLess(console.index('gt710_wait_probe::run();'), console.index('libbreenix::gpu::caps();'))
        self.assertNotIn('gt710_wait_probe::run();', gpu)
        selected = API['capture_categories']({'mode':'suite', 'suite':'gpu'})
        self.assertNotIn('recovery', selected)
        self.assertNotIn('contexts', selected)
        console_categories = API['capture_categories']({'mode':'suite', 'suite':'console'})
        self.assertIn('recovery', console_categories)
        self.assertIn('contexts', console_categories)


if __name__ == '__main__': unittest.main()
