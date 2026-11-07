"""D14's START suffix reaches native G28 reconstruction without changing pixels."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))

class PictureStart(unittest.TestCase):
    def test_native_picture_checker_accepts_producer_modes_and_refuses_other_suffixes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);pixels=bytes([0,255,0,255]);digest=hashlib.sha256(pixels).hexdigest()
            (root/'gt710-breengel-0000.rle').write_bytes(pixels)
            entry=dict(case='raster/fill-rect',picture=0,sha256=digest,bytes=4,width=1,height=1,encoding='raw')
            records=['GT710_BREENGL_PICTURE '+json.dumps(entry)]
            def serial(suffix):
                return (f'SUITE breengel START cases=1{suffix}\n'
                    +f'GT710_BREENGL_CHUNK 0 4 {digest}\n'
                    +f'SUITE breengel CASE raster/fill-rect PASS ms=1 sha256={digest}\n'
                    +f'GT710_FINAL_BULK 1 {"a"*64}\n'
                    +'GT710_BREENGL_TOTAL bytes=6 uploads=3 picture_bytes=4 pictures=1 unique=1\n'
                    +'SUITE_SEQUENCE READY breengel\n')
            for suffix in ['', ' screen_proof=on', ' pace_ms=1000 screen_proof=on']:
                API['breengel_picture_evidence'](root,serial(suffix),records,1,1)
            for suffix in [' screen_proof=off',' screen_proof=on screen_proof=on',' garbage=on',' pace_ms=0 screen_proof=on']:
                with self.assertRaisesRegex(ValueError,'picture upload sequence'):
                    API['breengel_picture_evidence'](root,serial(suffix),records,1,1)

if __name__=='__main__':unittest.main()
