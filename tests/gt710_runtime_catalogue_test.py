"""Score native/HiDPI latches using the capture's recorded EDID catalogue."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
MEMORY = runpy.run_path(str(ROOT / 'tests/gt710_memory_report_test.py'))
RAW = bytes.fromhex((ROOT / 'tests/fixtures/gt710-edid/lcd17-19.hex').read_text())
BOARD = dict(MEMORY['board'], edid=RAW.hex(), edid_sha256=hashlib.sha256(RAW).hexdigest(),
             modes=API['edid_timings'](RAW))
BOARD_TEXT = MEMORY['SERIAL'].split('GT710_NATIVE ')[0] + 'GT710_NATIVE ' + json.dumps(BOARD) + '\n'
MODES = [(1024, 768), (1280, 1024), (2560, 2048)]


def row(mode):
    w, h = MODES[mode]
    rw, rh = MODES[mode] if mode != 2 else MODES[1]
    address = 0x30000000 + mode * 0x2000000
    bank = [address >> 8, w | h << 16, 0x1000000 | w * 4, 0xcf00, 1]
    view = [0, 0, 0, w | h << 16, 0, rw | rh << 16, rw | rh << 16, rw | rh << 16]
    return dict(schema=1, sequence=mode + 1, mode=mode, resource=70 + mode, requested=address,
                head=0, pitch=w * 4, block_height=0, format=0, notifier=1,
                active=bank[:], armed=bank[:], viewport_active=view[:], viewport_armed=view[:],
                raster=[0, rw | rh << 16], firmware_writes=0)


def text(rows, prefix='GT710_DISPLAY_V1 '):
    return ''.join(prefix + json.dumps(r) + '\n' for r in rows)


class RuntimeCatalogue(unittest.TestCase):
    def test_new_catalogue_and_strict_latch_comparisons(self):
        check = API['runtime_display']
        rows = [row(i) for i in range(3)]
        self.assertEqual(check(BOARD_TEXT + text(rows), (1280, 1024)), rows)
        tiled = copy.deepcopy(rows)
        for r in tiled:
            r['block_height'] = 16
            r['active'][2] = r['armed'][2] = (r['pitch'] // 64) << 8 | 4
            r['active'][4] = r['armed'][4] = 3
        check(BOARD_TEXT + text(tiled), (1280, 1024))
        for mutate in [lambda r: r.update(requested=r['requested'] + 4096),
                       lambda r: r['active'].__setitem__(0, r['active'][0] + 16),
                       lambda r: r['armed'].__setitem__(0, r['armed'][0] + 16),
                       lambda r: r['active'].__setitem__(2, 0),
                       lambda r: r['active'].__setitem__(3, 0),
                       lambda r: r['viewport_active'].__setitem__(5, 1280 | 800 << 16),
                       lambda r: r['viewport_armed'].__setitem__(5, 1280 | 800 << 16),
                       lambda r: r['raster'].__setitem__(1, 1280 | 800 << 16),
                       lambda r: r.update(sequence=1),
                       lambda r: r.update(pitch=r['pitch'] + 256),
                       lambda r: r.update(firmware_writes=1),
                       lambda r: r.update(notifier=0)]:
            bad = copy.deepcopy(rows)
            mutate(bad[2])
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                check(BOARD_TEXT + text(bad), (1280, 1024))
        for serial in [text(rows), BOARD_TEXT + text(rows[:2]), BOARD_TEXT + text([row(1), row(0), row(2)])]:
            with self.assertRaises(ValueError):
                check(serial, (1280, 1024))
        with self.assertRaises(ValueError):
            check(BOARD_TEXT + text(rows), (1920, 1080))

    def test_console_allocation_uses_the_same_catalogue(self):
        rows = [row(1), row(0), row(2)]
        console = [rows[0]['requested'], 1280 * 1024 * 4]
        for sequence, r in enumerate(rows, 1):
            w, h = MODES[r['mode']]
            r.update(sequence=sequence, event='handoff' if sequence == 1 else 'mode',
                     resource=0xffffffff, console=console, allocation=[r['requested'], w * h * 4])
        serial = BOARD_TEXT + 'GT710_NATIVE_HANDOFF latch=1\n'
        memory = dict(total=1 << 30, reserved=[], objects=[])
        check = API['console_evidence']
        self.assertEqual(len(check(serial + text(rows, 'GT710_CONSOLE_V1 '), BOARD, memory)), 3)
        bad = copy.deepcopy(rows)
        bad[2]['allocation'][1] += 4096
        with self.assertRaises(ValueError):
            check(serial + text(bad, 'GT710_CONSOLE_V1 '), BOARD, memory)


if __name__ == '__main__':
    unittest.main()
