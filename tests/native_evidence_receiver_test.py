"""Replay native paging and wire lengths from the retained GPU proof."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from gt710_final_upload_test import Handler, upload

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = json.loads((ROOT/'tests/fixtures/native_gpu_upload_sequence.json').read_text())


class NativeEvidenceReceiver(unittest.TestCase):
    def replay(self, diagnostics_only=False, wire_only=False):
        with tempfile.TemporaryDirectory() as tmp:
            state=Path(tmp); boot=state/'boots/sample'; boot.mkdir(parents=True)
            (boot/'run.json').write_text(json.dumps({'screenProof':True}))
            with patch.object(upload.m,'STATE',state), patch.object(upload.m,'read_lease',return_value=dict(id='sample',state='powered')):
                route=upload.prepare(boot,'sample',{'BOOT_URL':'http://fixture.invalid/boot'},suite='gpu').split('fixture.invalid')[1]
                catalogue='GT710_EVIDENCE_MODES '+json.dumps({k:FIXTURE[k] for k in ('default','modes')})+'\n'
                sent=parts=crc=0
                for kind,length in FIXTURE['requests']:
                    if diagnostics_only and kind in ('screen','final'): continue
                    if wire_only and kind.startswith('crc-'): continue
                    body=(catalogue.encode()+bytes(length-len(catalogue))) if kind=='startup' else bytes(length)
                    suffix='' if kind=='startup' else '/final' if kind=='final' else '/diagnostics/'+kind
                    chunks=[body]
                    if kind=='screen': chunks=[body[i:i+upload.MAX_BYTES] for i in range(0,len(body),upload.MAX_BYTES)]
                    for chunk in chunks:
                        if kind=='screen': suffix=f'/screens/pictures/{parts}'; parts+=1
                        h=Handler(route+suffix,chunk); upload.receive(h)
                        self.assertEqual(h.code,200,f'{kind} after {crc} CRCs and {sent} accepted bytes')
                        sent+=len(chunk)
                    if kind.startswith('crc-'): crc+=1
                current=upload.m.read_json(state/'gt710-upload.json')
                self.assertEqual(crc,0 if wire_only else 237)
                self.assertEqual(len(current.get('diagnosticUploads',{})),0 if wire_only else 237)
                self.assertEqual(sum(current.get(k,0) for k in ('bytes','screenBytes','diagnosticBytes','finalBytes')),sent)
                if not diagnostics_only: self.assertTrue(current['finalComplete'])

    def test_runtime_paged_crc_inventory(self):
        self.replay(diagnostics_only=True)
        # 88 declared ends + 146 pages from the runtime-sized sources + 4 phases.
        self.assertEqual(upload.crc_checkpoint_cap('gpu', FIXTURE['modes'], FIXTURE['default']),238)
    def test_retained_upload_order_and_missing_body_upper_bounds(self): self.replay()

    def test_wire_volume_independent_of_crc_slots(self): self.replay(wire_only=True)

    def test_refusals_preserve_slots_and_bounds(self):
        with tempfile.TemporaryDirectory() as tmp:
            state=Path(tmp); boot=state/'boots/sample'; boot.mkdir(parents=True)
            (boot/'run.json').write_text(json.dumps({'screenProof':True}))
            with patch.object(upload.m,'STATE',state), patch.object(upload.m,'read_lease',return_value=dict(id='sample',state='powered')):
                route=upload.prepare(boot,'sample',{'BOOT_URL':'http://fixture.invalid/boot'},suite='gpu').split('fixture.invalid')[1]
                def send(suffix,body=b'row',digest=None):
                    h=Handler(route+suffix,body,digest); upload.receive(h); return h.code
                catalogue='GT710_EVIDENCE_MODES '+json.dumps({k:FIXTURE[k] for k in ('default','modes')})+'\n'
                self.assertEqual(send('',catalogue.encode()),200)
                self.assertEqual(send('/screens/pictures/1'),404)
                self.assertEqual(send('/diagnostics/unknown-checkpoint'),404)
                self.assertEqual(send('/screens/pictures/0',digest='0'*64),400)
                self.assertEqual(send('/screens/pictures/0',bytes(upload.MAX_BYTES+1)),400)
                self.assertEqual(send('/screens/pictures/0'),200)
                self.assertEqual(send('/screens/pictures/0'),404)
                current=upload.m.read_json(state/'gt710-upload.json')
                current['screenBytes']=upload.byte_bound(current)
                upload.m.write_json(state/'gt710-upload.json',current)
                self.assertEqual(send('/screens/pictures/1'),400)
                self.assertFalse((boot/'screen-part-0001.bin').exists())


if __name__=='__main__': unittest.main()
