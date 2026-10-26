"""The E19 capture cases reject the embedded display, incomplete facts and false provenance."""
import copy
import json
from pathlib import Path
import runpy
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
SHA = 'a' * 40
SERIAL = '''PCI_FN 01:00.0 10de:128b class=03/00 bar0=0x20000000/0x1000000 irq=0xff
PCI_DISPLAY 01:00.0 subsystem=1462:8c93 irq=0xff pin=1 bars=2
PCI_DISPLAY_BAR 01:00.0 index=0 address=0x20000000 size=0x1000000 kind=mem prefetch=0 width=32
PCI_DISPLAY_BAR 01:00.0 index=1 address=0x10000000 size=0x8000000 kind=mem prefetch=1 width=64
PCI_DISPLAY_LINK 01:00.0 cap=0x82 status=0x81 speed=1 width=8 max_speed=2 max_width=8
[devices] display found: pci 01:00.0 [10de:128b] subclass 0x00
[fbcon] framebuffer in display BAR: 1024x768 Bgr 4 bytes/pixel stride 1056 at 0x10000000 length 3244032, 01:00.0 10de:128b class 03/00 BAR1 0x10000000/0x8000000 prefetchable
[devices] display initialized: pci 01:00.0 [10de:128b] driver=firmware-framebuffer BAR1
'''


class CaptureCases(unittest.TestCase):
    def results(self, serial):
        return [c for cat in ('firmware', 'device') for c in API['cases'](cat, serial)]

    def test_console_requires_owner_readback_and_all_e16_cases_together(self):
        manifest = json.loads((ROOT / 'docs/suites/console.json').read_text())
        names = [cat['id'] + '/' + c['id'] for cat in manifest['categories'] for c in cat['cases']]
        serial = SERIAL + '[fbcon] screen read back identical: 768 pixel rows, 7 glyph cells match the font, sha256 ' + 'a' * 64 + '\n'
        serial += f'SUITE console START cases={len(names)}\n'
        serial += 'SUITE console MEMORY before free=100 heap_free_kb=10 user_pages=1\n'
        serial += ''.join('SUITE console CASE ' + name + ' PASS ms=1\n' for name in names)
        serial += f'SUITE console DONE passed={len(names)} failed=0 skipped=0 total={len(names)}\n'
        serial += 'SUITE console MEMORY after free=100 heap_free_kb=10 user_pages=1\n'
        serial += 'SUITE console MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=0\n'
        API['check_case']('console', 'acceptance', serial)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'definition'
            for name in ('scripts/metal/gt710-suite', 'scripts/suite-verdict.py',
                         'docs/suites/console.json', 'docs/boot-path.json'):
                dest = root / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / name, dest)
            retained = runpy.run_path(str(root / 'scripts/metal/gt710-suite'))
            retained['check_case']('console', 'acceptance', serial)
        for bad in (serial.replace('01:00.0 10de:128b class 03/00 BAR1', '04:00.0 102b:0536 class 03/00 BAR1'),
                    serial.replace('[fbcon] screen read back identical:', '[fbcon] mismatch:'),
                    serial.replace('getty/independent-sessions PASS ms=1', 'getty/independent-sessions FAIL ms=1 msg=refused'),
                    serial.replace('MEMORY reclaimed=PASS', 'MEMORY reclaimed=FAIL')):
            with self.assertRaises(ValueError):
                API['check_case']('console', 'acceptance', bad)

    def test_board_inventory_requires_live_edid_route_post_and_two_timings(self):
        import hashlib
        edid = bytearray(128)
        edid[:8] = bytes([0, 255, 255, 255, 255, 255, 255, 0])
        edid[18:21] = bytes([1, 4, 128])
        edid[38:40] = bytes([0x81, 0])
        edid[54:72] = bytes.fromhex('e26800a0a0402e603020360000000000001a')
        edid[127] = -sum(edid) & 255
        evidence = dict(schema=1, phase='board', bdf='01:00.0', boot0=0x106000a1,
                        chipset=0x106, revision=0xa1, post=2, rom_bytes=4096,
                        rom_sha256='a' * 64, bit_version=0x100, dcb_version=0x40,
                        i2c_version=0x41, connector_version=0x40, pll_version=0x40,
                        dcb_entry=0x01000102, dcb_conf=0x30, i2c_entry=0x3e2,
                        connector=0, connector_type=0x31, drive=2, sor=0,
                        sor_state=0x501, head=0, head_total=0x033f0540, head_clock=65000000,
                        ddc='live', edid=edid.hex(), edid_sha256=hashlib.sha256(edid).hexdigest(),
                        modes=[dict(width=1280, height=800, clock_khz=83500,
                                    hsync_start=1352, hsync_end=1480, htotal=1680,
                                    vsync_start=803, vsync_end=809, vtotal=831, flags=30),
                               dict(width=2560, height=1600, clock_khz=268500,
                                    hsync_start=2608, hsync_end=2640, htotal=2720,
                                    vsync_start=1603, vsync_end=1609, vtotal=1646, flags=26)])
        serial = lambda e: SERIAL + 'GT710_NATIVE ' + json.dumps(e) + '\n'
        API['check_case']('board', 'inventory', serial(evidence))
        for field, bad in [('ddc', 'firmware'), ('post', 0), ('chipset', 0x108),
                           ('drive', 3), ('connector', 1), ('sor_state', 0),
                           ('edid', edid[:-1].hex()), ('edid_sha256', 'b'*64),
                           ('modes', evidence['modes'][:1]), ('schema', 2)]:
            e = dict(evidence, **{field: bad})
            with self.subTest(field=field), self.assertRaises(ValueError):
                API['check_case']('board', 'inventory', serial(e))
        e = copy.deepcopy(evidence); e['modes'][0]['clock_khz'] += 1
        with self.assertRaises(ValueError): API['check_case']('board', 'inventory', serial(e))
        with self.assertRaises(ValueError): API['check_case']('board', 'inventory', serial(evidence)*2)

    def test_claim_requires_the_framebuffer_bar_index(self):
        with self.assertRaises(ValueError):
            API['check_case']('device', 'claimed', SERIAL.replace('driver=firmware-framebuffer BAR1', 'driver=firmware-framebuffer BAR0'))

    def test_padded_pitch_and_gt710_owner(self):
        self.assertTrue(all(c['result'] == 'PASS' for c in self.results(SERIAL)))

    def test_refuses_missing_length_wrong_owner_and_range(self):
        for old, new in [(' length 3244032', ''), ('01:00.0 10de:128b class 03/00 BAR1', '04:00.0 102b:0536 class 03/00 BAR1'),
                         ('length 3244032', 'length 134217729')]:
            with self.subTest(new=new):
                self.assertTrue(any(c['result'] == 'FAIL' for c in self.results(SERIAL.replace(old, new))))

    def test_refuses_missing_duplicate_or_mistranscribed_pci_facts(self):
        for line in SERIAL.splitlines()[:6]:
            with self.subTest(line=line):
                self.assertTrue(any(c['result'] == 'FAIL' for c in self.results(SERIAL.replace(line + '\n', ''))))
        for text in [SERIAL + SERIAL.splitlines()[0] + '\n', SERIAL.replace('speed=1 width=8', 'speed=2 width=8'),
                     SERIAL.replace('subsystem=1462:8c93', 'subsystem=0000:0000')]:
            self.assertTrue(any(c['result'] == 'FAIL' for c in self.results(text)))

    def test_score_recomputes_facts_and_requires_own_tools(self):
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        effort = json.loads((ROOT / 'docs/efforts/metal.json').read_text())
        boot = dict(id='fixture', toolsCommit=SHA, evidenceEligible=True, platform='metal', arch='x86_64', kind='boot', mode='console')
        records = dict(boot='fixture', commit=SHA, milestone='metal-gt710/device', cases=API['cases']('device', SERIAL))
        score = lambda r, b, s=SERIAL: API['score'](manifest, effort, r, s, b, SHA, ['KERNEL PANIC:'])
        score(records, boot)
        wrong = dict(boot, toolsCommit='b' * 40)
        with self.assertRaises(ValueError): score(records, wrong)
        with self.assertRaises(ValueError): score(records, boot, SERIAL.replace('subsystem=1462:8c93', 'subsystem=0000:0000'))
        missing = copy.deepcopy(records)
        missing['cases'].pop()
        with self.assertRaises(ValueError): score(missing, boot)
        with self.assertRaises(ValueError): score(records, boot, SERIAL + 'KERNEL PANIC: bad\n')


    def test_archive_requires_completed_clean_capture(self):
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        import metal_score
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            for name in ('docs/metal/boot-path.json', 'docs/boot-path.json', 'docs/efforts/metal.json',
                         'docs/suites/gt710.json', 'scripts/metal/gt710-suite',
                         'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
                dest = p / 'definition' / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / name, dest)
            boot = dict(id='fixture', toolsCommit=SHA, evidenceEligible=True, platform='metal', arch='x86_64',
                        kind='boot', mode='console', disk='root')
            for name, value in [('run.json', json.dumps(boot)), ('serial.txt', SERIAL), ('events.jsonl', ''),
                                ('commit', SHA), ('dirty', '0'), ('exit-status', '0'), ('ended', '1')]:
                (p / name).write_text(value)
            for cat in ('firmware', 'device'):
                records = dict(boot='fixture', commit=SHA, milestone='metal-gt710/' + cat, cases=API['cases'](cat, SERIAL))
                (p / ('gt710-' + cat + '.json')).write_text(json.dumps(records))
            result = lambda: metal_score.score(p)['milestones']
            self.assertTrue(result()['metal-gt710/firmware']['passed'])
            self.assertTrue(result()['metal-gt710/device']['passed'])
            for name, value in [('dirty', '1'), ('exit-status', '137'), ('events.jsonl', '{"event":"console-gap"}\n')]:
                original = (p / name).read_text()
                (p / name).write_text(value)
                self.assertFalse(result()['metal-gt710/device']['passed'])
                (p / name).write_text(original)
            (p / 'ended').unlink()
            self.assertFalse(result()['metal-gt710/device']['passed'])


if __name__ == '__main__':
    unittest.main()
