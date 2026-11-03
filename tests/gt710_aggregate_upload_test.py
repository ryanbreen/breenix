"""Native suite evidence stays separate while a sequence shares one boot lease."""
import tempfile
from pathlib import Path
from urllib.parse import urlsplit
import unittest
from unittest.mock import patch
from gt710_final_upload_test import Handler, upload


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


if __name__ == '__main__':
    unittest.main()
