"""Exercise standalone evidence slots using a non-metal state backend."""
import hashlib
import json
from contextlib import nullcontext
from pathlib import Path
import tempfile
import unittest

from gt710_final_upload_test import Handler, upload


class Backend:
    def __init__(self, root):
        self.STATE = Path(root)

    def read_lease(self):
        return {'id': 'sample', 'state': 'powered'}

    def boot_dir(self, boot_id):
        return self.STATE / 'boots' / boot_id

    def locked(self, name):
        return nullcontext()

    def read_json(self, path, default=None):
        return json.loads(path.read_text()) if path.exists() else default

    def write_json(self, path, value):
        path.write_text(json.dumps(value))


class EvidenceBackendTest(unittest.TestCase):
    def test_standalone_ui_testing_upload(self):
        with tempfile.TemporaryDirectory() as tmp:
            backend = Backend(tmp)
            boot = backend.boot_dir('sample')
            boot.mkdir(parents=True)
            route = '/gt710-upload/' + 'a' * 64
            backend.write_json(backend.STATE / 'gt710-upload.json',
                               dict(id='sample', route=route, suite='ui-testing'))
            ready = Handler(route, b'')
            upload.ready(ready, backend)
            self.assertEqual(ready.code, 204)
            parts = (b'picture one', b'picture two')
            body = b''.join(parts)
            digest = hashlib.sha256(body).hexdigest()
            manifest = f'SUITE_PARTS v=1 first=0 parts=2 bytes={len(body)} sha256={digest}\n'.encode()

            def send(suffix, payload, sha=None):
                handler = Handler(route + suffix, payload, sha)
                upload.receive(handler, backend)
                return handler.code

            self.assertEqual(send('', manifest), 200)
            self.assertEqual(send('/pictures/1', parts[1]), 404)
            self.assertEqual(send('/pictures/0', parts[0], '0' * 64), 400)
            for index, part in enumerate(parts):
                self.assertEqual(send(f'/pictures/{index}', part), 200)
                self.assertEqual((boot / f'gt710-breengel-{index:04}.rle').read_bytes(), part)
            self.assertEqual(send('/diagnostics/uitest-0123456789abcdef', b'diagnostic'), 200)
            self.assertEqual(send('/final', b'final'), 200)
            self.assertEqual(send('/pictures/2', b'late'), 404)
            self.assertEqual((boot / 'gt710-pixels.rle').read_bytes(), manifest)
            self.assertEqual((boot / 'diagnostic-uitest-0123456789abcdef.bin').read_bytes(), b'diagnostic')
            self.assertEqual((boot / 'gt710-final.rle').read_bytes(), b'final')


if __name__ == '__main__':
    unittest.main()
