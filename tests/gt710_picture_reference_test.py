"""Repeated picture chunks retain canonical payloads while using bounded wire references."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from gt710_final_upload_test import Handler, upload


class PictureReferences(unittest.TestCase):
    def exercise(self, mutation=None):
        with tempfile.TemporaryDirectory() as tmp:
            state=Path(tmp);boot=state/'boots/sample';boot.mkdir(parents=True)
            (boot/'run.json').write_text(json.dumps(dict(screenProof=True)))
            with patch.object(upload.m,'STATE',state),patch.object(upload.m,'read_lease',return_value=dict(id='sample',state='powered')):
                route='/gt710-upload/'+'a'*64
                upload.m.write_json(state/'gt710-upload.json',dict(id='sample',route=route,suite='breengel',complete=True))
                body=b'pattern'*100
                first=Handler(route+'/pictures/0',body);upload.receive(first);self.assertEqual(first.code,200)
                digest=hashlib.sha256(body).hexdigest()
                reference=f'PICTURE_REF v=1 source=0 bytes={len(body)} sha256={digest}\n'.encode()
                if mutation:reference=mutation(boot,reference)
                second=Handler(route+'/pictures/1',reference)
                second.headers['X-Breenix-Picture-Reference']='1'
                upload.receive(second)
                if mutation:
                    self.assertEqual(second.code,400)
                    self.assertFalse((boot/'gt710-breengel-0001.rle').exists())
                else:
                    self.assertEqual(second.code,200)
                    self.assertEqual((boot/'gt710-breengel-0001.rle').read_bytes(),body)
                    receipt=upload.m.read_json(boot/'native-picture-upload-0001.json')
                    self.assertEqual((receipt['bytes'],receipt['sha256']),(len(body),digest))
                    self.assertEqual(receipt['wireBytes'],len(reference))
                    intent=upload.m.read_json(state/'gt710-upload.json')
                    self.assertEqual(intent['pictureBytes'],len(body)+len(reference))
                    self.assertEqual(intent['picturePayloadBytes'],2*len(body))

    def test_reference_round_trip_preserves_payload_and_counts_wire_bytes(self):
        self.exercise()

    def test_reference_refuses_missing_corrupt_foreign_forward_and_wrong_digest(self):
        def missing(boot,body):
            (boot/'gt710-breengel-0000.rle').unlink();return body
        def corrupt(boot,body):
            (boot/'gt710-breengel-0000.rle').write_bytes(b'bad');return body
        def foreign(boot,body):
            p=boot/'native-picture-upload-0000.json';d=json.loads(p.read_text());d['id']='other';p.write_text(json.dumps(d));return body
        def forward(boot,body):return body.replace(b'source=0',b'source=1')
        def wrong(boot,body):return body.replace(b'sha256=',b'sha256=0')
        for mutation in [missing,corrupt,foreign,forward,wrong]:
            with self.subTest(mutation=mutation.__name__):self.exercise(mutation)

    def test_reference_refuses_nonproof_and_unsealed_sources(self):
        def production(boot,body):
            (boot/'run.json').write_text(json.dumps(dict(screenProof=False)));return body
        def unsealed(boot,body):
            p=boot/'native-picture-upload-0000.json';d=json.loads(p.read_text());d['complete']=False;p.write_text(json.dumps(d));return body
        for mutation in [production,unsealed]:
            with self.subTest(mutation=mutation.__name__):self.exercise(mutation)

    def test_repeated_large_parts_fit_wire_budget_with_identical_retained_payloads(self):
        def run(references):
            with tempfile.TemporaryDirectory() as tmp:
                state=Path(tmp);boot=state/'boots/sample';boot.mkdir(parents=True)
                (boot/'run.json').write_text(json.dumps(dict(screenProof=True)))
                with patch.object(upload.m,'STATE',state),patch.object(upload.m,'read_lease',return_value=dict(id='sample',state='powered')):
                    route='/gt710-upload/'+'a'*64
                    upload.m.write_json(state/'gt710-upload.json',dict(id='sample',route=route,suite='breengel',complete=True,
                        bytes=200000,screenBytes=49<<20,diagnosticBytes=100000))
                    for index in range(8):
                        source=index if index<6 else index-6
                        payload=bytes([source])*(2<<20)
                        body=payload
                        if references and index>=6:
                            body=f'PICTURE_REF v=1 source={source} bytes={len(payload)} sha256={hashlib.sha256(payload).hexdigest()}\n'.encode()
                        h=Handler(route+f'/pictures/{index}',body)
                        if body is not payload:h.headers['X-Breenix-Picture-Reference']='1'
                        upload.receive(h)
                        if not references and h.code==400:return
                        self.assertEqual(h.code,200)
                        self.assertEqual((boot/f'gt710-breengel-{index:04}.rle').read_bytes(),payload)
                    self.assertTrue(references,'literal duplicate transfers unexpectedly fit')
                    final=Handler(route+'/final',b'final'*((2<<20)//5))
                    upload.receive(final);self.assertEqual(final.code,200)
                    intent=upload.m.read_json(state/'gt710-upload.json')
                    self.assertEqual(intent['picturePayloadBytes'],16<<20)
                    self.assertLess(intent['pictureBytes'],(12<<20)+300)
        run(False)
        run(True)

    def test_reference_does_not_bypass_wire_or_materialized_payload_limits(self):
        def full(boot,body):
            p=upload.m.STATE/'gt710-upload.json';d=json.loads(p.read_text());d['picturePayloadBytes']=upload.MAX_TOTAL;p.write_text(json.dumps(d));return body
        self.exercise(full)
        def wire_full(boot,body):
            p=upload.m.STATE/'gt710-upload.json';d=json.loads(p.read_text());d['bytes']=upload.MAX_TOTAL;p.write_text(json.dumps(d));return body
        self.exercise(wire_full)

if __name__=='__main__':unittest.main()
