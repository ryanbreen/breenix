# Hardware encodings from pinned MIT sources; see THIRD_PARTY_NOTICES.
# /*
#  * Copyright 2010 Red Hat Inc.
#  * Copyright 2013 Red Hat Inc.
#  * Copyright 2011 Red Hat Inc.
#  * Copyright 2012 Red Hat Inc.
#  * Copyright 2016 Red Hat Inc.
#  *
#  * Permission is hereby granted, free of charge, to any person obtaining a
#  * copy of this software and associated documentation files (the "Software"),
#  * to deal in the Software without restriction, including without limitation
#  * the rights to use, copy, modify, merge, publish, distribute, sublicense,
#  * and/or sell copies of the Software, and to permit persons to whom the
#  * Software is furnished to do so, subject to the following conditions:
#  *
#  * The above copyright notice and this permission notice shall be included in
#  * all copies or substantial portions of the Software.
# claim-lint:ok: MIT license condition; source inventory in vendor/kepler/sources.json.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  *
#  * Authors: Ben Skeggs
#  */
# /*******************************************************************************
#     Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.
#
#     Permission is hereby granted, free of charge, to any person obtaining a
#     copy of this software and associated documentation files (the "Software"),
#     to deal in the Software without restriction, including without limitation
#     the rights to use, copy, modify, merge, publish, distribute, sublicense,
#     and/or sell copies of the Software, and to permit persons to whom the
#     Software is furnished to do so, subject to the following conditions:
#
#     The above copyright notice and this permission notice shall be included in
#     all copies or substantial portions of the Software.
# claim-lint:ok: MIT license condition; G1 source policy in tools/kepler/README.md.
#
#     THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#     IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#     FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#     THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
#     LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
#     FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
#     DEALINGS IN THE SOFTWARE.
#
# *******************************************************************************/
"""M8.1 host scoring: native evidence, negative controls and a named refusal boot."""
import hashlib
import json
from pathlib import Path
import runpy
import struct
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
SEQUENCE = runpy.run_path(str(ROOT / 'scripts/suite-sequence.py'))
SHA = 'a' * 40

def caps_bytes():
    data = bytearray(512)
    def put(offset, *words): struct.pack_into('<' + 'I' * len(words), data, offset, *words)
    put(0, 1, 512, 2, 3, 6, 512, 0xa197, 0, 4)
    put(36, 0x927d, 0xa06f, 0xa0b5, 0xa197)
    put(68, 0, 0, 1, 0, 2, 0, 0, 0, 16384, 16384, 1, 0, 1)
    struct.pack_into('<Q', data, 120, 3 | (1 << 3) | (2 << 30))
    data[128:134] = b'GK208B'
    reason = b'missing b8g8r8a8-render'
    data[192:192+len(reason)] = reason
    put(256, 1, 1280, 1024, 0)
    return data

def records():
    result = [dict(schema=1, type='caps', bytes=512, hex=caps_bytes().hex())]
    sequence = 5
    def add(case, control='clean'):
        nonlocal sequence
        sequence += 1
        masked = control == 'masked-class'
        r = dict(schema=1, type='completion', case=case, control=control,
                 context=0x123000, epoch=17, sequence=0 if masked else sequence,
                 caller=(1 << 32) | sequence, resource=0x456000 if case == 'triangle' else 0x800000,
                 generation=1, kind=0xa1971b0c if case == 'triangle' else 0xa0b50300,
                 answer=0x60000000 if control == 'clean' else 0,
                 observed_context=0 if masked else 0x123000,
                 observed_epoch=0 if masked else (16 if control == 'stale-epoch' else 17),
                 observed_sequence=0 if masked or control == 'withheld-release' else sequence,
                 consumed=0 if masked else sequence, healthy=not masked, elapsed_us=100,
                 outcome='done' if control == 'clean' else ('refused' if masked else 'rejected'))
        result.append(r)
        if masked: sequence -= 1
    for _ in range(5): add('triangle')
    for control, after in [('withheld-release','after-withheld'), ('stale-epoch','after-stale'), ('masked-class','after-mask')]:
        add('copy', control); add(after)
    add('copy')
    return result

def stage(directory, rows=None):
    rows = records() if rows is None else rows
    body = ''.join('GT710_CORE ' + json.dumps(row, separators=(',',':')) + '\n' for row in rows).encode()
    anchor = f'{len(body)} {hashlib.sha256(body).hexdigest()}\n'
    (directory / 'gt710-pixels.rle').write_bytes(body)
    (directory / 'serial.txt').write_text('GT710_CORE_BULK ' + anchor + 'GT710_BULK ' + anchor)
    return API['capture_serial'](directory)

def gpu_boot():
    return dict(id='20261028T180000Z-metal', platform='metal', arch='x86_64', kind='boot',
                evidenceEligible=True, mode='suite', suite='gpu', suites=['gpu'],
                toolsCommit=SHA, bootedCommit=SHA)

def gpu_serial():
    manifest = json.loads((ROOT / 'docs/suites/gpu.json').read_text())
    names = [cat['id']+'/'+case['id'] for cat in manifest['categories'] for case in cat['cases']]
    reason = 'missing the GPU core with a device admitted to the profile (G1): CAPS status 2 (missing b8g8r8a8-render)'
    return ('SUITE gpu START cases=170\n' + ''.join(f'SUITE gpu CASE {name} FAIL ms=1 msg={reason}\n' for name in names)
            + 'SUITE gpu DONE passed=0 failed=170 skipped=0 total=170\nSUITE gpu ELAPSED ms=210\nSUITE_SEQUENCE READY gpu\n')

class CoreReport(unittest.TestCase):
    def joined(self, rows=None):
        with tempfile.TemporaryDirectory() as tmp: return stage(Path(tmp), rows)

    def test_native_row_and_dispatch_pass(self):
        serial = self.joined()
        API['core_row'](serial)
        API['core_dispatch'](serial)

    def test_wire_row_rejects_virtual_or_invented_capabilities(self):
        mutations = [(16,4), (16,5), (20,3), (24,1), (28,1), (32,3), (40,0xa16f),
                     (52,1), (72,3), (92,7), (96,330), (100,32768), (108,2), (112,1),
                     (120,7), (124,2), (256+12,1), (272,1), (8,0)]
        for offset, value in mutations:
            with self.subTest(offset=offset, value=value):
                rows=records(); data=caps_bytes(); struct.pack_into('<I', data, offset, value)
                rows[0]['hex']=data.hex()
                with self.assertRaises(ValueError): API['core_row'](self.joined(rows))
        for offset in [134,215,511]:
            rows=records(); data=caps_bytes(); data[offset]=1; rows[0]['hex']=data.hex()
            with self.subTest(offset=offset), self.assertRaises(ValueError): API['core_row'](self.joined(rows))

    def test_controls_virtual_answers_and_identity_fail(self):
        changes = [(6,'outcome','done'), (6,'answer',0x60000000), (6,'observed_sequence',11),
                   (8,'observed_epoch',17), (10,'sequence',15), (10,'outcome','done'),
                   (7,'answer',0), (9,'outcome','rejected'), (11,'healthy',False),
                   (1,'answer',0x1100), (1,'answer',0), (1,'kind',0x0200),
                   (1,'kind',1141), (12,'kind',0x0205), (12,'kind',1102),
                   (2,'caller',7), (3,'observed_context',0x124000), (4,'observed_epoch',16),
                   (5,'consumed',0), (12,'elapsed_us',2000001)]
        for index,key,value in changes:
            rows=records(); rows[index][key]=value
            with self.subTest(index=index,key=key), self.assertRaises(ValueError): API['core_dispatch'](self.joined(rows))
        for index in [1,6,7,8,9,10,11,12]:
            rows=records(); rows.pop(index)
            with self.subTest(missing=index), self.assertRaises(ValueError): API['core_dispatch'](self.joined(rows))

    def test_missing_or_changed_bulk_anchor_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp); stage(path)
            original=(path/'serial.txt').read_text()
            for changed in [original.replace('GT710_CORE_BULK','REMOVED'), original.replace('GT710_BULK','REMOVED'),
                            original.replace(original.split()[2], '0'*64), original+original.splitlines()[0]+'\n']:
                (path/'serial.txt').write_text(changed)
                with self.assertRaises(ValueError): API['core_row'](API['capture_serial'](path))

    def test_named_producer_requires_complete_refusals_in_manifest_order(self):
        serial=self.joined()
        API['core_producer'](gpu_serial(), API['core_row'](serial), gpu_boot())
        text=gpu_serial()
        changes=[text.replace(' FAIL ', ' SKIP ',1), text.replace(' FAIL ', ' PASS ',1),
                 text.replace('b8g8r8a8-render','3d-feature'), text.replace('CAPS status 2','CAPS status 1'),
                 text.replace('ms=1 msg=','ms=1 msg=another failure; ',1),
                 text.replace('SUITE gpu START cases=170\n',''), text.replace('SUITE gpu DONE','SUITE other DONE'),
                 text.replace('SUITE gpu ELAPSED ms=210\n',''), text.replace('ms=210','ms=900001'),
                 text.replace('SUITE_SEQUENCE READY gpu\n',''), text+'KERNEL PANIC: broken\n',
                 text+'SUITE_SEQUENCE STALLED gpu cases=170/170\n']
        lines=text.splitlines(keepends=True); lines[1],lines[2]=lines[2],lines[1]; changes.append(''.join(lines))
        for changed in changes:
            with self.subTest(capture=changed[:100]), self.assertRaises(ValueError):
                API['core_producer'](changed, API['core_row'](serial), gpu_boot())
        for field,value in [('suite','console'), ('mode','persistent'), ('toolsCommit','b'*40), ('evidenceEligible',False)]:
            boot=gpu_boot();boot[field]=value
            with self.subTest(field=field),self.assertRaises(ValueError): API['core_producer'](text,API['core_row'](serial),boot)

    def test_core_manifest_runner_and_effort_mapping(self):
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text()); API['validate_manifest'](manifest)
        category=next(c for c in manifest['categories'] if c['id']=='core')
        self.assertEqual([c['id'] for c in category['cases']],['row','dispatch','producer'])
        self.assertEqual(category.get('hostRunner'),True)
        effort=json.loads((ROOT/'docs/efforts/metal.json').read_text())
        milestone=next(m for e in effort['efforts'] if e['id']=='metal-gt710' for m in e['milestones'] if m['id']=='gpu-core')
        self.assertEqual(milestone['measure'],dict(suite='gt710',categories=['core']))
        self.assertTrue(SEQUENCE['gpu_3d']('x86-metal'))
        self.assertEqual(SEQUENCE['resolve']('gpu','x86-metal',SEQUENCE['gpu_3d']('x86-metal'))[0],['gpu'])
        # The build's producer list and the actual image placement rule, on both architectures.
        self.assertIn('"suite-gpu"',(ROOT/'userspace/programs/build.sh').read_text())
        self.assertEqual(next(line.split()[2] for line in (ROOT/'libs/libbreenix/src/layout.tsv').read_text().splitlines() if line.startswith('bin suite-* ')), 'sbin')

if __name__ == '__main__': unittest.main()
