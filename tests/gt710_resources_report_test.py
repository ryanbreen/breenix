# Hardware encodings from pinned MIT sources; see THIRD_PARTY_NOTICES.
# Copyright 2010 Christoph Bumiller
# Copyright (c) 2019 NVIDIA CORPORATION. All rights reserved.
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
# Copyright (c) 1993-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
"""M8 PR 8.2: score production resources evidence, never diagnostic core rows."""
import hashlib
import json
from pathlib import Path
import runpy
import shutil
import struct
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
SHA = 'a' * 40
MANIFEST = json.loads((ROOT / 'docs/suites/gpu.json').read_text())
NAMES = [c['id'] + '/' + t['id'] for c in MANIFEST['categories'] for t in c['cases']]
EXCLUDED = {'device/device-error-reported', 'readback/owner-only',
            'readback/error-invalidates-targets', 'textures/quota',
            'textures/immutable-while-pinned', 'textures/destroy-deferred'}
PIPELINES = ['solid-rects', 'rect-batch', 'copy', 'copy-clip', 'copy-scaled',
             'nearest-scale', 'box-filter', 'clip-nested', 'order']
REQUIRED = [n for n in NAMES if (n.split('/')[0] in ('device', 'readback', 'textures')
            and n not in EXCLUDED) or n in ['pipelines/' + p for p in PIPELINES]]


def caps_bytes():
    data = bytearray(512)
    def put(offset, *words):
        struct.pack_into('<' + 'I' * len(words), data, offset, *words)
    put(0, 1, 512, 0, 0, 6, 512, 0xa197, 0, 4)
    put(36, 0x927d, 0xa06f, 0xa0b5, 0xa197)
    put(68, 0, 3, 1, 3, 2, 1, 7, 0, 16384, 16384, 1, 0, 1)
    origins = [3, 1] + [4] * 8 + [2]
    struct.pack_into('<Q', data, 120, sum(o << (3 * i) for i, o in enumerate(origins)))
    data[128:134] = b'GK208B'
    put(256, 1, 1280, 1024, 0)
    return data


PROBE_FILES = ['colour.bgra', 'colour.bgra', 'colour.rgba', 'colour.bgra',
               'r8.bgra', 'integer.bgra', 'colour.bgra', 'region.bgra']


def probes():
    result = []
    for cap, name in enumerate(PROBE_FILES, 3):
        expected = (ROOT / 'kernel/src/gpu/gk208/caps' / name).read_bytes()
        result.append(dict(schema=1, phase='caps-probe', capability=cap, width=80, height=16,
                           completed=True, exact=True, error='', expected_sha256=hashlib.sha256(expected).hexdigest(),
                           first=expected.hex(), second=expected.hex()))
    return result


def stage(directory, data=None, probe_rows=None):
    data = caps_bytes() if data is None else data
    row = ('GT710_RESOURCES ' + json.dumps(dict(schema=1, type='caps', bytes=512,
                                              hex=data.hex()), separators=(',', ':')) + '\n').encode()
    anchor = f'{len(row)} {hashlib.sha256(row).hexdigest()}\n'
    rows = probes() if probe_rows is None else probe_rows
    body = b''; markers = ''
    for probe in rows:
        raw = json.dumps(probe, separators=(',', ':')).encode()
        body += raw + b'\n'
        markers += f'GT710_CAPS_PROBE id={probe["capability"]} sha256={hashlib.sha256(raw).hexdigest()}\n'
    body += row
    (directory / 'gt710-pixels.rle').write_bytes(body)
    (directory / 'serial.txt').write_text(markers + 'GT710_RESOURCES_BULK ' + anchor
        + f'GT710_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n')
    return API['capture_serial'](directory)


def boot(suite='gpu'):
    return dict(id='20261029T120000Z-metal' if suite == 'gpu' else '20261029T121000Z-metal',
                platform='metal', arch='x86_64', kind='boot', mode='suite', suite=suite,
                suites=[suite], evidenceEligible=True, toolsCommit=SHA, bootedCommit=SHA,
                build=dict(commit=SHA, profile='production', features=[]))


def transcript(suite='gpu', all_pass=False):
    names = NAMES if suite == 'gpu' else [c['id'] + '/' + t['id'] for c in
        json.loads((ROOT / 'docs/suites/console.json').read_text())['categories'] for t in c['cases']]
    passed = sum(suite == 'console' or all_pass or n in REQUIRED for n in names)
    text = f'SUITE {suite} START cases={len(names)}\n'
    text += f'SUITE {suite} MEMORY before free=1000 heap_free_kb=500 user_pages=100\n'
    for n in names:
        result = 'PASS' if suite == 'console' or all_pass or n in REQUIRED else 'FAIL'
        text += f'SUITE {suite} CASE {n} {result} ms=1' + (' msg=not implemented' if result == 'FAIL' else '') + '\n'
    text += (f'SUITE {suite} DONE passed={passed} failed={len(names)-passed} skipped=0 total={len(names)}\n'
             f'SUITE {suite} MEMORY after free=1000 heap_free_kb=500 user_pages=100\n'
             f'SUITE {suite} MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=64\n'
             f'SUITE {suite} ELAPSED ms=900000\nSUITE_SEQUENCE READY {suite}\n'
             'USERSPACE TEST REPORT DONE\n')
    return text


def events(seconds=330):
    return [dict(event='handoff', seconds=10), dict(event='final-marker', seconds=10 + seconds)]


class ResourcesReport(unittest.TestCase):
    def test_step_list_and_manifest_use_shared_case_ids(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-8-2.json').read_text())
        self.assertEqual(step['cases'], REQUIRED)
        self.assertEqual(len(REQUIRED), 53)
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        API['validate_manifest'](manifest)
        cat = next(c for c in manifest['categories'] if c['id'] == 'resources')
        self.assertTrue(cat['hostRunner'])
        self.assertEqual([c['id'] for c in cat['cases']], ['row', 'suite', 'console'])

    def test_admitted_row_and_missing_probe_class_scanout_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            API['resources_row'](stage(d))
            for offset, value in [(8, 2), (12, 3), (16, 4), (20, 3), (24, 1),
                                  (28, 1), (32, 3), (36, 0), (52, 1), (72, 1),
                                  (88, 0), (88, 2), (88, 3), (92, 3), (96, 330), (100, 32768),
                                  (108, 2), (116, 0), (256, 0), (260, 0), (268, 1), (272, 1)]:
                data = caps_bytes(); struct.pack_into('<I', data, offset, value)
                with self.subTest(offset=offset), self.assertRaises(ValueError):
                    API['resources_row'](stage(d, data))
            for i in range(11):
                data = caps_bytes()
                bits = int.from_bytes(data[120:128], 'little') & ~(7 << (3 * i))
                data[120:128] = bits.to_bytes(8, 'little')
                with self.subTest(origin=i+1), self.assertRaises(ValueError):
                    API['resources_row'](stage(d, data))
            for offset in [134, 192, 511, 127]:
                data = caps_bytes(); data[offset] = 1
                with self.subTest(reserved=offset), self.assertRaises(ValueError):
                    API['resources_row'](stage(d, data))

    def test_bulk_requires_uploaded_bytes_and_both_digests(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp); joined = stage(d)
            serial = (d / 'serial.txt').read_text(); body = (d / 'gt710-pixels.rle').read_bytes()
            changes = [serial.replace('GT710_RESOURCES_BULK', 'MISSING'),
                       serial.replace('GT710_BULK', 'MISSING'), serial + serial.splitlines()[0] + '\n',
                       serial.replace(hashlib.sha256(body).hexdigest(), '0'*64, 1),
                       body.decode() + serial]
            for text in changes:
                (d / 'serial.txt').write_text(text)
                with self.subTest(text=text[:60]), self.assertRaises(ValueError):
                    API['resources_row'](API['capture_serial'](d))
            (d / 'serial.txt').write_text(serial)
            (d / 'gt710-pixels.rle').write_bytes(body.replace(b'GK208B', b'OTHER!') + b' ')
            with self.assertRaises(ValueError): API['capture_serial'](d)
            self.assertIn('GT710_RESOURCES ', joined)

    def test_probe_images_are_complete_exact_and_digest_bound(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            for key, value in [('completed', False), ('exact', False), ('first', ''),
                               ('second', '00'), ('error', 'failed'), ('expected_sha256', '0'*64)]:
                rows = probes(); rows[0][key] = value
                with self.subTest(key=key), self.assertRaises(ValueError):
                    API['resources_row'](stage(d, probe_rows=rows))
            for rows in [probes()[:-1], probes()[::-1], []]:
                with self.subTest(missing=len(rows)), self.assertRaises(ValueError):
                    API['resources_row'](stage(d, probe_rows=rows))
            stage(d)
            serial = (d/'serial.txt').read_text()
            digest = serial.splitlines()[0].split('sha256=')[1]
            (d/'serial.txt').write_text(serial.replace(digest, '0'*64, 1))
            with self.assertRaises(ValueError): API['capture_serial'](d)

    def test_suite_records_all_results_but_requires_only_step_passes(self):
        result = API['resources_suite'](transcript(), boot())
        self.assertEqual(result['elapsed_ms'], 900000)
        self.assertEqual([c['id'] for c in result['cases']], NAMES)
        self.assertEqual(sum(c['result'] == 'PASS' for c in result['cases']), 53)
        self.assertEqual(sum(c['result'] == 'FAIL' for c in result['cases']), 117)
        self.assertIn('not implemented', next(c['reason'] for c in result['cases'] if c['result'] == 'FAIL'))
        API['resources_suite'](transcript(all_pass=True), boot())
        for name in REQUIRED:
            changed = transcript().replace(f'CASE {name} PASS ms=1', f'CASE {name} FAIL ms=1 msg=broken')
            with self.subTest(case=name), self.assertRaises(ValueError):
                API['resources_suite'](changed, boot())

    def test_incomplete_skipped_misordered_fatal_memory_and_time_fail(self):
        text = transcript()
        lines = text.splitlines(keepends=True); lines[2], lines[3] = lines[3], lines[2]
        changes = [''.join(lines), text.replace('START cases=170', 'START cases=169'),
                   text.replace('SUITE gpu CASE device/version PASS ms=1\n', ''),
                   text.replace('FAIL ms=1 msg=not implemented', 'SKIP msg=not implemented', 1),
                   text.replace('failed=117', 'failed=116'), text.replace('skipped=0', 'skipped=1'),
                   text.replace('SUITE gpu ELAPSED ms=900000\n', ''), text.replace('ms=900000', 'ms=900001'),
                   text.replace('ms=900000', 'ms=0'), text.replace('MEMORY before', 'MEMORY absent'),
                   text.replace('free_after=1000', 'free_after=10'), text.replace('reclaimed=PASS', 'reclaimed=FAIL'),
                   text.replace('SUITE_SEQUENCE READY gpu\n', ''),
                   text+'SUITE_SEQUENCE STALLED gpu cases=170/170\n', text+'KERNEL PANIC: broken\n',
                   text+'SUITE gpu UNKNOWN data\n', text.rstrip('\n')]
        for changed in changes:
            with self.subTest(text=changed[-100:]), self.assertRaises(ValueError):
                API['resources_suite'](changed, boot())

    def test_production_identity_requires_no_features_injection_or_other_commit(self):
        for key, value in [('kind', 'persistent'), ('mode', 'probe'), ('suite', 'console'),
                           ('suites', ['gpu', 'console']), ('toolsCommit', 'b'*40),
                           ('evidenceEligible', False), ('inject', 'anything'), ('arch', 'aarch64')]:
            changed = dict(boot(), **{key: value})
            with self.subTest(field=key), self.assertRaises(ValueError):
                API['resources_suite'](transcript(), changed)
        for key, value in [('profile', 'gpu-core'), ('features', ['testing']), ('commit', 'b'*40)]:
            changed = dict(boot(), build=dict(boot()['build'], **{key: value}))
            with self.subTest(build=key), self.assertRaises(ValueError):
                API['resources_suite'](transcript(), changed)

    def test_console_requires_88_passes_and_handoff_bound(self):
        self.assertEqual(API['resources_console'](transcript('console'), boot('console'), events()), 330000)
        for ev in [events(330.001), events(0), events(-1), events()+[events()[0]],
                   events()+[dict(event='console-gap')], events(float('nan'))]:
            with self.subTest(events=ev), self.assertRaises(ValueError):
                API['resources_console'](transcript('console'), boot('console'), ev)
        for text in [transcript('console').replace('PASS ms=1', 'FAIL ms=1 msg=bad', 1),
                     transcript('console').replace('passed=88', 'passed=87'),
                     transcript('console')+'KERNEL PANIC: broken\n']:
            with self.assertRaises(ValueError): API['resources_console'](text, boot('console'), events())

    def test_capture_binds_two_completed_clean_boots_and_report(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp); gpu = d / 'resources-producer'; gpu.mkdir()
            for path, suite in [(d, 'console'), (gpu, 'gpu')]:
                row = stage(path) if suite == 'gpu' else ''
                serial = (path / 'serial.txt').read_text() if row else ''
                (path / 'serial.txt').write_text(serial + transcript(suite))
                for name, value in {'run.json': json.dumps(boot(suite)), 'commit': SHA,
                                    'dirty': '0', 'exit-status': '1' if suite == 'gpu' else '0', 'ended': '1',
                                    'events.jsonl': ''.join(json.dumps(e)+'\n' for e in events())}.items():
                    (path / name).write_text(value)
            producer = API['resources_capture'](d, SHA)
            self.assertEqual([c['result'] for c in API['cases']('resources', '', producer)], ['PASS']*3)
            self.assertEqual(API['record_resources_capture'](d, SHA), 0)
            report = json.loads((d / 'gt710-resources.json').read_text())
            self.assertEqual(report['producerBoot'], boot()['id'])
            self.assertEqual(len(report['gpuCases']), 170)
            manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
            effort = json.loads((ROOT / 'docs/efforts/metal.json').read_text())
            self.assertIn('3 cases passed', API['score'](manifest, effort, report, transcript('console'),
                boot('console'), SHA, [], producer))
            for key, value in [('producerBoot', boot('console')['id']), ('gpuCases', []),
                               ('gpuElapsedMS', 1), ('consoleHandoffToDoneMS', 1)]:
                with self.subTest(report=key), self.assertRaises(ValueError):
                    API['score'](manifest, effort, dict(report, **{key: value}), transcript('console'),
                                 boot('console'), SHA, [], producer)
            # Exercise the normal scorer from the retained build definition,
            # including the new step list and the source probe expectations.
            files = ['docs/metal/boot-path.json', 'docs/boot-path.json', 'docs/efforts/metal.json',
                     'docs/suites/gt710.json', 'docs/suites/gpu.json', 'docs/suites/console.json',
                     'docs/gui/gt710-step-8-2.json', 'scripts/metal/gt710-suite', 'scripts/suite-verdict.py',
                     'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json']
            files += ['kernel/src/gpu/gk208/caps/' + n for n in set(PROBE_FILES)]
            for name in files:
                target = d/'definition'/name; target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT/name, target)
            metal = runpy.run_path(str(ROOT/'scripts/metal/metal_score.py'))
            measured = metal['score'](d)['milestones']['metal-gt710/resources']
            self.assertEqual((measured['passed'], measured['reached'], measured['total']), (True, 3, 3))
            for name, value in [('dirty', '1'), ('commit', 'b'*40), ('ended', ''), ('exit-status', '124'),
                                ('events.jsonl', json.dumps(dict(event='console-gap'))+'\n')]:
                old = (gpu/name).read_text(); (gpu/name).write_text(value)
                with self.subTest(file=name), self.assertRaises(ValueError): API['resources_capture'](d, SHA)
                (gpu/name).write_text(old)
            (gpu/'run.json').write_text(json.dumps(dict(boot(), id=boot('console')['id'])))
            with self.assertRaises(ValueError): API['resources_capture'](d, SHA)


if __name__ == '__main__':
    unittest.main()
