"""Bulk transport preserves the existing pixel oracle and refuses bad framing."""
import hashlib
import io
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts/metal'))
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
class Bulk(unittest.TestCase):
    def test_bulk_is_required_and_bound_to_the_serial_digest(self):
        with tempfile.TemporaryDirectory() as temporary:
            d=Path(temporary);body=b'GT710_PIX 0 1:ff003039\n'
            report='GT710_MODE {"schema":1}\n'
            serial=report+'GT710_BULK '+str(len(body))+' '+hashlib.sha256(body).hexdigest()+'\n'
            (d/'serial.txt').write_text(serial)
            with self.assertRaises((ValueError,OSError)):API['capture_serial'](d)
            (d/'gt710-pixels.rle').write_bytes(body)
            self.assertEqual(API['capture_serial'](d),body.decode()+serial)
            for bad in [body[:-1],body.replace(b'3039',b'3038'),body+body]:
                (d/'gt710-pixels.rle').write_bytes(bad)
                with self.assertRaises(ValueError):API['capture_serial'](d)
    def test_rows_keep_their_original_update_boundaries(self):
        reports=['GT710_MODE {"schema":1}\n','GT710_SWITCH {"step":0}\n',
                 'GT710_SWITCH {"step":1}\n','GT710_FLIP {"mode":0,"step":0}\n']
        rows=['GT710_PIX 0 1:ff000001\n','GT710_SWITCH_PIX 0 0 1:ff000002\n',
              'GT710_SWITCH_PIX 1 0 1:ff000003\n','GT710_FLIP_PIX 0 0 0 1:ff000004\n']
        with tempfile.TemporaryDirectory() as temporary:
            d=Path(temporary)
            def stage(body,metadata):
                (d/'gt710-pixels.rle').write_bytes(body)
                digest='GT710_BULK '+str(len(body))+' '+hashlib.sha256(body).hexdigest()+'\n'
                (d/'serial.txt').write_text(''.join(metadata)+digest)
                return digest
            body=''.join(rows).encode();digest=stage(body,reports)
            self.assertEqual(API['capture_serial'](d),''.join(r+m for r,m in zip(rows,reports))+digest)
            for body,metadata in [(''.join(rows[::-1]).encode(),reports),
                                  (''.join(rows).encode(),reports[:-1]),
                                  (''.join(rows).encode(),reports+reports[:1])]:
                stage(body,metadata)
                with self.assertRaises(ValueError):API['capture_serial'](d)

    def test_bounded_upload_requires_live_lease_exact_body_and_digest(self):
        import gt710_upload as upload
        class Connection:
            def settimeout(self,value):pass
        class Handler:
            def __init__(self,body,length=None,sha=None):
                self.path='/gt710-upload/'+'a'*64;self.rfile=io.BytesIO(body);self.connection=Connection()
                self.headers={'Content-Length':str(len(body) if length is None else length),
                    'X-Content-SHA256':sha or hashlib.sha256(body).hexdigest()}
                self.code=None
            def send_error(self,code,*args):self.code=code
            def send_response(self,code):self.code=code
            def send_header(self,*args):pass
            def end_headers(self):pass
        with tempfile.TemporaryDirectory() as temporary:
            state=Path(temporary);boot=state/'boots'/'sample';boot.mkdir(parents=True)
            intent=dict(id='sample',route='/gt710-upload/'+'a'*64)
            def reset():
                (state/'gt710-upload.json').write_text(json.dumps(intent))
                for name in ('gt710-pixels.rle','gt710-pixels.rle.partial','native-upload.json'):
                    (boot/name).unlink(missing_ok=True)
            with patch.object(upload.m,'STATE',state),patch.object(upload.m,'read_lease',return_value=dict(id='sample',state='powered')):
                reset();h=Handler(b'actual rows\n');upload.receive(h);self.assertEqual(h.code,200)
                self.assertEqual((boot/'gt710-pixels.rle').read_bytes(),b'actual rows\n')
                h=Handler(b'actual rows\n');upload.receive(h);self.assertEqual(h.code,404)
                for length,sha in [(0,None),(upload.MAX_BYTES+1,None),(99,None),(None,'0'*64)]:
                    reset();h=Handler(b'actual rows\n',length,sha);upload.receive(h)
                    self.assertEqual(h.code,400);self.assertFalse((boot/'gt710-pixels.rle').exists())
                reset()
                with patch.object(upload.m,'read_lease',return_value=dict(id='other',state='powered')):
                    h=Handler(b'rows');upload.receive(h);self.assertEqual(h.code,404)
if __name__=='__main__':unittest.main()
