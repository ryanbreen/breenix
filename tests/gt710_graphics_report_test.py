# Hardware encodings from pinned MIT sources; see THIRD_PARTY_NOTICES.
# /*
#  * Copyright 2010 Red Hat Inc.
#  * Copyright 2011 Red Hat Inc.
#  * Copyright 2013 Red Hat Inc.
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
"""M7.2 consumes raw graphics facts and retained complementary context images."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
HASHES = {
    'fecs.code.bin': 'b3d192b4423fe1e150bd66b819374724ddb2ba9c40f92e4c83aba03139b85d3f',
    'fecs.data.bin': '3d4dc9a6c009e844ce7d538496cb267ad78984f0256ccfc29c73412a446806a6',
    'gpccs.code.bin': 'ccc302340f23f6624bd278038b696b27eb20cc5a256547ea9a244aff6e3164b6',
    'gpccs.data.bin': '67859e7b5e6e4002910d5ae94d2668828c867a7d0ef1787624dd97cc8839b405',
}
PCI = 'PCI_FN 01:00.0 10de:128b class=03/00 bar0=0x80000000/0x1000000 irq=0x10\n'
# G7 synthetic sections: HUB MMIO, HUB strand, GPC MMIO, GPC strand.
RANGES = [(256,384),(512,640),(768,896),(1024,1152)]
DATA = bytes((i * 31 + i // 7) & 255 for i in range(512))
def make_image(poison):
    image = bytearray([poison]) * 1280
    for n,(start,end) in enumerate(RANGES):
        image[start:end] = DATA[n*128:(n+1)*128]
    return bytes(image)
IMAGES = tuple(make_image(p) for p in [85,170])
IMAGE = IMAGES[0]
def saved_data(image):
    return b''.join(image[a:b] for a,b in RANGES)


def fixture():
    return dict(
        schema=1, bdf='01:00.0', chipset=262,
        firmware={k: dict(sha256=v, readback_sha256=v) for k, v in HASHES.items()},
        status=[[0x409800, 0x80000000], [0x502800, 0x80000000]],
        elapsed_us=12000, bound_us=2000000, fecs_size=len(IMAGE),
        queries=[dict(method=m, poison=p, reply=p | 0x80000000,
                      size=len(IMAGE), elapsed_us=100, bound_us=2000000)
                 for m, p in [(1, 0x15555555), (2, 0x15555555), (1, 0x2aaaaaaa), (2, 0x2aaaaaaa)]],
        withheld_start=dict(started=False, ready=False, poison=0x55555555,
                            reply=0x55555555, accepted=False),
        stale_reply=dict(method=1, poison=0, planted=0x80000000,
                         reply=0, accepted=False),
        topology_raw=[[0x409604, 0x10001], [0x502608, 1], [0x500c30, 1]],
        topology=dict(gpc_nr=1, tpc_nr=[1], tpc_mask=[1]), golden_bytes=len(IMAGE),
        saved_end=1152, save_layout=dict(gpc=0, strands=1, strand_base=4, strand_words=32,
            units=[dict(controller=0x409000, mmio_base=1, mmio_words=32, strands=[dict(base=2,words=32)]),
                   dict(controller=0x502000, mmio_base=3, mmio_words=32, strands=[dict(base=4,words=32)])]),
        runnable_header=dict(count=18, va=0x540000,
                             words=[18, 0x5400],
                             sha256=hashlib.sha256(int(18).to_bytes(4, 'little') + int(0x5400).to_bytes(4, 'little') + bytes(248)).hexdigest()),
        image_bulk_bytes=len(image_rows()), bulk_artifact_estimate=len(image_rows()),
        saves=[dict(poison=p, va=0x400000 + n * 0x1000,
                    sha256=hashlib.sha256(DATA).hexdigest(), image_sha256=hashlib.sha256(IMAGES[n]).hexdigest()) for n, p in enumerate([85, 170])],
        init_lists=[
            dict(name='mmio', count=115, sha256='7e944a30c8d9f35bfefef91d0a73e4c1135510ad2615265af141cf75ced4e79e'),
            dict(name='icmd', count=245, sha256='e062254f9033263c696cb5f66ff2cd150f0ce771022fc6021cb696ec72195ff2'),
            dict(name='mthd', count=311, sha256='4004603ade727d25413eae88f9f796b1e0e3780a21f467095e54008b050a2041'),
            dict(name='hub', count=95, sha256='436d91aea0155fb03f1a402dff9d34c7849d9076f6e27e1663364e145976b5b5'),
            dict(name='gpc_0', count=28, sha256='cb9bd415e525aff81bb331a6dc07a302226baa5265baa9709123be009c847739'),
            dict(name='gpc_1', count=18, sha256='26ff9fdc13e750bbf4a6e773333d38af7ddb50ca75f0bc4c53c6e507a2764ee7'),
            dict(name='tpc', count=44, sha256='d9f5c076e64f442d3edfca20b8e11ca27b1dafad8b64dd477f8eed933963eb90'),
            dict(name='ppc', count=14, sha256='47d9ecd1b081049e83745541d580984c5a00b6290ef71ee5ed2a3a2c6ff35968'),
            dict(name='zcull', count=24, sha256='6627431341d439972b5f1ec0442db9fc79ff21ba084cc79061288c03f9b1681e'),
        ],
        global_buffers=[dict(name='bundle', va=0x500000, bytes=0x3000, align=0x100),
                        dict(name='pagepool', va=0x504000, bytes=0x8000, align=0x100),
                        dict(name='attrib', va=0x510000, bytes=0x16460, align=0x1000),
                        dict(name='mmio', va=0x540000, bytes=0x1000, align=0x100)],
        channel=dict(id=0, epoch=42, instance=0x10000000, class_id=0xa197,
                     top=[0x80000036, 3] + [0] * 62, engine=0, runlist=0,
                     bind=0x80010000, state=1, current_context=0x80010000),
        releases=[dict(class_id=0xa197, method=0x1b0c, word=0x1000f010,
                       target=0x600000, poison=p, nonce=0x12345678,
                       observed=0x12345678, epoch=42, elapsed_us=100, bound_us=2000000)
                  for p in [85, 170]],
        withheld_release=dict(poison=85, observed=85, accepted=False),
        stale_release=dict(epoch=41, nonce=0x12345678, observed=0x12345678, accepted=False),
        faults=[[0x400100, 0], [0x400108, 0], [0x259c, 0]],
        teardown=dict(runlist_count=0, binding=0), baseline=[1, 2, 3], after=[1, 2, 3],
        firmware_writes=0, draw='pending-7.4', shaders='pending-7.3', context_switch='pending-7.5')


def image_rows(images=IMAGES):
    return ''.join(f'GT710_GR_IMAGE {poison} {offset} {data[offset:offset+64].hex()}\n'
                   for poison, data in zip([85, 170], images)
                   for offset in range(0, len(data), 64)).encode('ascii')


def stage(directory, record, body=None):
    body = image_rows() if body is None else body
    anchor = f'GT710_GR_IMAGE_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n'
    outer = f'GT710_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n'
    serial = PCI + 'GT710_GR ' + json.dumps(record) + '\n' + anchor + outer
    (directory / 'serial.txt').write_text(serial)
    (directory / 'gt710-pixels.rle').write_bytes(body)
    return serial


class Graphics(unittest.TestCase):
    def check(self, record):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            stage(directory, record)
            API['graphics_engine'](API['capture_serial'](directory))

    def test_clean_capture_passes_three_manifest_cases_and_console_route(self):
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        self.assertEqual(manifest['categories'][0]['id'], 'graphics')
        self.assertEqual(list(API['CATEGORIES'])[0], 'graphics')
        API['validate_manifest'](manifest)
        effort = json.loads((ROOT / 'docs/efforts/metal.json').read_text())
        milestone = next(m for e in effort['efforts'] if e['id'] == 'metal-gt710'
                         for m in e['milestones'] if m['id'] == 'graphics')
        self.assertEqual(milestone['measure'], dict(suite='gt710', categories=['graphics']))
        self.assertIn('graphics', API['capture_categories'](dict(mode='suite', suite='console')))
        self.assertNotIn('graphics', API['capture_categories'](dict(mode='probe')))
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            stage(directory, fixture())
            serial = API['capture_serial'](directory)
            API['graphics_engine'](serial)
            cases = API['cases']('graphics', serial)
            self.assertEqual([c['id'] for c in cases],
                             ['graphics/ctxctl', 'graphics/context', 'graphics/channel'])
            self.assertEqual([c['result'] for c in cases], ['PASS'] * 3)

    def test_tpc_units_word_keeps_zcull_count_out_of_topology(self):
        record = fixture()
        # MIT nouveau gpc.fuc:149-156: UNITS TPC count is bits 4:0.
        record['topology_raw'][1][1] = 0x00010001
        self.check(record)

    def test_pinned_hashes_match_the_7_1_inventory(self):
        inventory = json.loads((ROOT / 'docs/gui/provenance/kepler-toolchain.sha256.json').read_text())
        for name, digest in HASHES.items():
            self.assertEqual(inventory[name], digest)
        self.assertEqual(API['GR_FIRMWARE_HASHES'], HASHES)

    def test_each_raw_fact_and_negative_control_has_a_specific_failure(self):
        mutations = [
            ('firmware hash', lambda e: e['firmware']['fecs.code.bin'].update(sha256='f' * 64)),
            ('firmware read-back', lambda e: e['firmware']['gpccs.data.bin'].update(readback_sha256='f' * 64)),
            ('firmware ready', lambda e: e['status'][0].__setitem__(1, 0)),
            ('firmware ready', lambda e: e['status'][1].__setitem__(1, 0)),
            ('mailbox reply', lambda e: e['queries'][0].update(reply=e['queries'][0]['poison'], accepted=True)),
            ('withheld start', lambda e: e['withheld_start'].update(accepted=True)),
            ('stale reply', lambda e: e['stale_reply'].update(accepted=True)),
            ('raw topology', lambda e: e['topology_raw'][1].__setitem__(1, 2)),
            ('golden size', lambda e: e.update(golden_bytes=len(IMAGE) + 4)),
            ('complementary images', lambda e: e['saves'][1].update(sha256='f' * 64)),
            ('current context', lambda e: e['channel'].update(current_context=0x80020000)),
            ('release class', lambda e: e['releases'][0].update(class_id=0xa06f)),
            ('release nonce', lambda e: e['releases'][0].update(observed=85)),
            ('withheld release', lambda e: e['withheld_release'].update(accepted=True)),
            ('stale release', lambda e: e['stale_release'].update(accepted=True)),
            ('trap/fault', lambda e: e['faults'][0].__setitem__(1, 1)),
            ('trap/fault', lambda e: e['faults'][1].__setitem__(1, 1)),
            ('trap/fault', lambda e: e['faults'][2].__setitem__(1, 1)),
            ('allocator baseline', lambda e: e.update(after=[1, 2, 4])),
            ('firmware writes', lambda e: e.update(firmware_writes=1)),
            ('graphics identity', lambda e: e.update(bdf='02:00.0')),
            ('graphics identity', lambda e: e.update(chipset=0x108)),
            ('firmware bound', lambda e: e.update(elapsed_us=2000001)),
            ('mailbox size', lambda e: e['queries'][0].update(size=0)),
            ('mailbox method', lambda e: e['queries'][0].update(method=0x10)),
            ('mailbox bound', lambda e: e['queries'][0].update(elapsed_us=2000001)),
            ('init lists', lambda e: e['init_lists'][0].update(count=0)),
            ('init lists', lambda e: e['init_lists'][0].update(sha256='0' * 64)),
            ('init lists', lambda e: e['init_lists'][0].update(count=1)),
            ('init lists', lambda e: e['init_lists'][0].update(name='unknown')),
            ('init lists', lambda e: e['init_lists'].pop()),
            ('init lists', lambda e: e['init_lists'].reverse()),
            ('global buffers', lambda e: e['global_buffers'][1].update(va=0x500000)),
            ('GR runlist', lambda e: e['channel'].update(runlist=2)),
            ('graphics class', lambda e: e['channel'].update(class_id=0xa06f)),
            ('release operation', lambda e: e['releases'][0].update(word=0x1000f011)),
            ('release bound', lambda e: e['releases'][0].update(elapsed_us=2000001)),
            ('channel teardown', lambda e: e['teardown'].update(binding=1)),
            ('pending markers', lambda e: e.update(draw='done')),
        ]
        self.check(fixture())
        for reason, mutate in mutations:
            with self.subTest(reason=reason):
                bad = copy.deepcopy(fixture())
                mutate(bad)
                with self.assertRaisesRegex(ValueError, reason):
                    self.check(bad)

    def test_g6_rejects_changed_saved_data_header_padding_and_runnable_header(self):
        # Refresh transport/full/region hashes so rejection comes from bytes,
        # rather than from a stale hash. The firmware map defines the G7 byte classification.
        for offset in [0,255,256,383,384,511,512,639,640,767,768,895,896,1023,1024,1151,1152,1279]:
            with self.subTest(offset=offset), tempfile.TemporaryDirectory() as temporary:
                e = fixture()
                changed = bytearray(IMAGES[1]); changed[offset] ^= 1
                e['saves'][1]['image_sha256'] = hashlib.sha256(changed).hexdigest()
                e['saves'][1]['sha256'] = hashlib.sha256(saved_data(changed)).hexdigest()
                body = image_rows((IMAGES[0], changed))
                e['image_bulk_bytes'] = len(body); e['bulk_artifact_estimate'] = len(body)
                directory = Path(temporary); stage(directory, e, body)
                with self.assertRaisesRegex(ValueError, 'complementary images|poison outside data'):
                    API['graphics_engine'](API['capture_serial'](directory))
        for key, value in [('saved_end', 892), ('runnable_header', dict(count=17, va=0x540000, words=[17, 0x5400], sha256='0' * 64))]:
            e = fixture(); e[key] = value
            with self.assertRaisesRegex(ValueError, 'saved size words|runnable header'):
                self.check(e)

    def test_g7_rejects_incomplete_overlapping_or_shifted_firmware_maps(self):
        for mutate in [lambda e:e['save_layout']['units'].pop(),
                       lambda e:e['save_layout']['units'][0]['strands'].clear(),
                       lambda e:e['save_layout']['units'][0].update(mmio_words=31),
                       lambda e:e['save_layout']['units'][1]['strands'][0].update(base=3),
                       lambda e:e['save_layout']['units'][1].update(mmio_base=2)]:
            e=fixture(); mutate(e)
            with self.assertRaisesRegex(ValueError,'saved size words|poison outside data|image digest'):
                self.check(e)

    def test_bulk_bytes_framing_and_image_digests_are_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            serial = stage(directory, fixture())
            body = image_rows()
            (directory / 'gt710-pixels.rle').write_bytes(body.replace(b'001f', b'001e', 1))
            with self.assertRaisesRegex(ValueError, 'display artifact differs'):
                API['capture_serial'](directory)
            # Rehashing the transport must still fail the independently recorded save digest.
            changed = IMAGES[1][:300] + bytes([IMAGES[1][300] ^ 1]) + IMAGES[1][301:]
            stage(directory, fixture(), image_rows((IMAGES[0], changed)))
            with self.assertRaisesRegex(ValueError, 'image digest'):
                API['graphics_engine'](API['capture_serial'](directory))
            stage(directory, fixture())
            (directory / 'serial.txt').write_text(serial.replace('GT710_BULK ', 'REMOVED_BULK '))
            with self.assertRaisesRegex(ValueError, 'graphics image bulk missing'):
                API['capture_serial'](directory)
            stage(directory, fixture())
            (directory / 'gt710-pixels.rle').unlink()
            with self.assertRaisesRegex(ValueError, 'graphics image bulk missing'):
                API['capture_serial'](directory)
            stage(directory, fixture())
            (directory / 'serial.txt').write_text(serial.replace('GT710_GR_IMAGE_BULK ', 'REMOVED_IMAGE_BULK '))
            with self.assertRaisesRegex(ValueError, 'update order'):
                API['capture_serial'](directory)
            stage(directory, fixture())
            anchor = next(line for line in serial.splitlines() if line.startswith('GT710_GR_IMAGE_BULK '))
            (directory / 'serial.txt').write_text(serial.replace(anchor, anchor[:-64] + '0' * 64))
            with self.assertRaisesRegex(ValueError, 'graphics image artifact differs'):
                API['capture_serial'](directory)
            rows = body.splitlines(keepends=True)
            interleaved = rows[0] + b'GT710_MULTI {}\n' + b''.join(rows[1:])
            stage(directory, fixture(), interleaved)
            with self.assertRaisesRegex(ValueError, 'interleaved'):
                API['capture_serial'](directory)
            stage(directory, fixture(), b''.join(rows[1:]))
            with self.assertRaisesRegex(ValueError, 'image rows'):
                API['graphics_engine'](API['capture_serial'](directory))


if __name__ == '__main__':
    unittest.main()
