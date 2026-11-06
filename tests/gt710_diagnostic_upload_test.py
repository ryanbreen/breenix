"""Bounded diagnostic blobs use authenticated slots without consuming pictures."""
import hashlib
import tempfile
from pathlib import Path
from unittest.mock import patch
import unittest
from gt710_final_upload_test import Handler, upload


class DiagnosticUpload(unittest.TestCase):
    def test_diagnostic_is_digest_checked_one_use_and_precedes_final(self):
        with tempfile.TemporaryDirectory() as tmp:
            state=Path(tmp);boot=state/'boots/sample';boot.mkdir(parents=True)
            with patch.object(upload.m,'STATE',state), patch.object(upload.m,'read_lease',return_value=dict(id='sample',state='powered')):
                route=upload.prepare(boot,'sample',{'BOOT_URL':'http://fixture.invalid/boot'},suite='window-manager').split('fixture.invalid')[1]
                def send(suffix,body=b'trace',sha=None):
                    h=Handler(route+suffix,body,sha);upload.receive(h);return h.code
                self.assertEqual(send('/diagnostics/pipeline'),404)
                self.assertEqual(send(''),200)
                body=b'WMPIPE01'+bytes(128)
                self.assertEqual(send('/diagnostics/pipeline',body),200)
                self.assertEqual((boot/'diagnostic-pipeline.bin').read_bytes(),body)
                receipt=upload.m.read_json(boot/'native-diagnostic-pipeline.json')
                self.assertEqual(receipt['sha256'],hashlib.sha256(body).hexdigest())
                self.assertEqual(send('/diagnostics/pipeline',b'replacement'),404)
                self.assertEqual(send('/diagnostics/bad',body,'0'*64),400)
                self.assertFalse((boot/'diagnostic-bad.bin').exists())
                self.assertEqual(send('/pictures/0',b'picture'),200)
                self.assertEqual(send('/final',b'final'),200)
                self.assertEqual(send('/diagnostics/late'),404)

    def test_diagnostic_preserves_request_and_boot_byte_bounds(self):
        with tempfile.TemporaryDirectory() as tmp:
            state=Path(tmp);boot=state/'boots/sample';boot.mkdir(parents=True)
            with patch.object(upload.m,'STATE',state), patch.object(upload.m,'read_lease',return_value=dict(id='sample',state='powered')):
                route=upload.prepare(boot,'sample',{'BOOT_URL':'http://fixture.invalid/boot'},suite='window-manager').split('fixture.invalid')[1]
                h=Handler(route,b'startup');upload.receive(h);self.assertEqual(h.code,200)
                h=Handler(route+'/diagnostics/large',bytes(upload.MAX_BYTES+1));upload.receive(h);self.assertEqual(h.code,400)
                intent=upload.m.read_json(state/'gt710-upload.json');intent['bytes']=upload.MAX_TOTAL-4;upload.m.write_json(state/'gt710-upload.json',intent)
                h=Handler(route+'/diagnostics/end',b'12345');upload.receive(h);self.assertEqual(h.code,400)
                self.assertFalse((boot/'diagnostic-end.bin').exists())


if __name__=='__main__':unittest.main()
