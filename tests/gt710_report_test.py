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
        return [c for cat in API['CATEGORIES'] for c in API['cases'](cat, serial)]

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
            for cat in API['CATEGORIES']:
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
