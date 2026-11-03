"""Native suite evidence stays separate while a sequence shares one boot lease."""
import hashlib
import json
import runpy
import tempfile
from pathlib import Path
from urllib.parse import urlsplit
import unittest
from unittest.mock import patch
from gt710_final_upload_test import Handler, upload, ROOT

API = runpy.run_path(str(ROOT / "scripts/metal/gt710-suite"))


class AggregateUpload(unittest.TestCase):
    def test_console_and_breengel_keep_independent_ordered_slots(self):
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            boot = state / 'boots/sample'
            boot.mkdir(parents=True)
            with patch.object(upload.m, 'STATE', state), patch.object(
                    upload.m, 'read_lease', return_value=dict(id='sample', state='powered')):
                route = urlsplit(upload.prepare(boot, 'sample', {'BOOT_URL': 'http://fixture.invalid/boot'},
                                                suite='console,time,breengel,files-io')).path
                def send(suite, suffix='', body=b'rows\n'):
                    handler = Handler(route + '/suites/' + suite + suffix, body)
                    upload.receive(handler)
                    return handler.code
                self.assertEqual(send('console'), 200)
                self.assertEqual(send('console', '/pictures/0'), 404)
                self.assertEqual(send('console', '/final', b'console final\n'), 200)
                self.assertEqual(send('breengel', '/pictures/0'), 404)
                self.assertEqual(send('breengel', body=b'breengel startup\n'), 200)
                self.assertEqual(send('breengel', '/pictures/0', b'picture\n'), 200)
                self.assertEqual(send('breengel', '/pictures/0'), 404)
                self.assertEqual(send('breengel', '/pictures/2'), 404)
                self.assertEqual(send('breengel', '/final', b'breengel final\n'), 200)
                self.assertEqual(send('breengel', '/pictures/1'), 404)
                self.assertEqual(send('gpu'), 404)
                self.assertEqual((boot / 'native-suites/console/gt710-final.rle').read_bytes(), b'console final\n')
                self.assertEqual((boot / 'native-suites/breengel/gt710-final.rle').read_bytes(), b'breengel final\n')

    def test_reprobe_uses_authenticated_images_from_the_earlier_suite(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            first = dict(phase='caps-probe', capability=3, first='1122', second='3344')
            late = dict(phase='caps-reprobe', capability=3, epoch=2,
                        first_sha256=hashlib.sha256(bytes.fromhex(first['first'])).hexdigest(),
                        second_sha256=hashlib.sha256(bytes.fromhex(first['second'])).hexdigest())
            def stage(suite, row):
                path = directory / 'native-suites' / suite
                path.mkdir(parents=True, exist_ok=True)
                body = (json.dumps(row, separators=(',', ':')) + '\n').encode()
                (path / 'gt710-pixels.rle').write_bytes(body)
                probe = ('GT710_CAPS_PROBE id=3' if suite == 'console'
                         else 'GT710_CAPS_REPROBE id=3 epoch=2')
                return (probe + ' sha256=' + hashlib.sha256(body.rstrip(b'\n')).hexdigest() + '\n'
                        + f'GT710_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n'
                        + f'SUITE {suite} START cases=1\nSUITE {suite} CASE test/draw PASS ms=1\n'
                        + f'SUITE {suite} DONE passed=1 failed=0 skipped=0 total=1\n'
                        + f'SUITE_SEQUENCE READY {suite}\n')
            original = stage('console', first)
            suffix = stage('gpu', late)
            (directory / 'serial.txt').write_text(original + suffix)
            decoded = API['capture_serial'](directory, 'gpu')
            row = next(json.loads(line) for line in decoded.splitlines() if line.startswith('{'))
            self.assertEqual((row['first'], row['second']), ('1122', '3344'))
            self.assertNotIn('"phase":"caps-probe"', decoded)
            # A payload with intact framing but different original pixels is rejected.
            changed = stage('console', dict(first, first='1123'))
            (directory / 'serial.txt').write_text(changed + suffix)
            with self.assertRaisesRegex(ValueError, 'reprobe digest differs'):
                API['capture_serial'](directory, 'gpu')
            (directory / 'serial.txt').write_text(original + suffix)
            (directory / 'native-suites/console/gt710-pixels.rle').write_bytes(b'changed')
            with self.assertRaises(ValueError):
                API['capture_serial'](directory, 'gpu')

    def test_scorer_selects_suite_boundaries_and_keeps_actual_boot_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / 'native-suites/breengel').mkdir(parents=True)
            serial = ('console startup\nSUITE console START cases=88\nSUITE_SEQUENCE READY console\n'
                      'breengel startup\nSUITE breengel START cases=401\nSUITE breengel CASE a/b PASS ms=1\n'
                      'SUITE_SEQUENCE READY breengel\nSUITE files-io START cases=1\n')
            (directory / 'serial.txt').write_text(serial)
            with patch.dict(API['capture_serial'].__globals__, capture_serial_body=lambda path, text: (path, text)):
                path, text = API['capture_serial'](directory, 'breengel')
                self.assertEqual(path, directory / 'native-suites/breengel')
                self.assertIn('breengel startup', text)
                self.assertIn('SUITE breengel CASE a/b PASS', text)
                self.assertNotIn('console', text)
                self.assertNotIn('files-io', text)
                with self.assertRaises(ValueError):
                    API['capture_serial'](directory)
                with self.assertRaises(ValueError):
                    API['capture_serial'](directory, 'gpu')
            from gt710_breengel_selection_test import boot
            identity = boot()
            identity.update(suite=None, suites=['console','time','breengel','files-io'])
            API['resources_identity'](identity, 'breengel')
            with self.assertRaises(ValueError):
                API['resources_identity'](identity, 'gpu')
            with self.assertRaises(ValueError):
                API['resources_identity'](dict(identity, suite='breengel'), 'breengel')


if __name__ == '__main__':
    unittest.main()
