"""G21 requires console receipts and allocations to follow the G20 latch."""
import copy
from functools import lru_cache
import json
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
DISPLAY = runpy.run_path(str(ROOT / 'tests/gt710_display_evidence_test.py'))
MEMORY = runpy.run_path(str(ROOT / 'tests/gt710_memory_report_test.py'))


@lru_cache(maxsize=None)
def capture(kind):
    path, function = {
        'mode': ('gt710_native_report_test.py', 'capture'),
        'switches': ('gt710_switch_report_test.py', 'switch_capture'),
        'flips': ('gt710_flip_report_test.py', 'capture'),
    }[kind]
    return runpy.run_path(str(ROOT / 'tests' / path))[function]()


def logical_console():
    row = DISPLAY['row'](1)
    row.update(resource=0xffffffff, requested=0x30000000)
    row['active'][0] = row['armed'][0] = row['requested'] >> 8
    return row


def console_event(sequence=1, event='handoff', mode=1):
    row = DISPLAY['row'](mode)
    row.update(sequence=sequence, event=event, resource=0xffffffff,
               requested=0x30000000 if mode else 0x32000000,
               console=[0x30000000, 2560*1600*4])
    row['active'][0] = row['armed'][0] = row['requested'] >> 8
    row['allocation'] = [row['requested'], row['pitch'] * (1600 if mode else 800)]
    return row


def event_text(rows):
    return ''.join('GT710_CONSOLE_V1 '+json.dumps(r)+'\n' for r in rows)


class G21Receipts(unittest.TestCase):
    def test_e19_surface_cannot_remain_the_console_after_a_g20_latch(self):
        # The receipt is deliberately stale even though firmware writes stay 0.
        # Existing MODE/SWITCH/FLIP pixels and completion records stay intact.
        for kind, name in [('mode', 'native_mode'),
                           ('switches', 'native_switches'),
                           ('flips', 'native_flips')]:
            serial = capture(kind)
            memory = API['display_memory'](serial)
            old = memory['objects'][0]['start']
            serial += DISPLAY['text']([logical_console()])
            serial += f'GT710_NATIVE_CONSOLE surface={old} firmware_writes=0\n'
            with self.subTest(check=name), self.assertRaises(ValueError):
                API[name](serial)

    def test_logical_console_needs_its_own_exact_allocation_evidence(self):
        # GT710_MEMORY describes the E19 allocations only. A logical latch and
        # receipt do not establish the extent of a different allocation.
        row = logical_console()
        serial = capture('mode') + DISPLAY['text']([row])
        serial += f'GT710_NATIVE_CONSOLE surface={row["requested"]} firmware_writes=0\n'
        with self.assertRaises(ValueError):
            API['display_memory'](serial)

    def test_logical_console_receipts_require_exact_allocations_and_measured_latches(self):
        serial = capture('mode')
        good = console_event()
        receipt = f'GT710_NATIVE_CONSOLE surface={good["requested"]} firmware_writes=0 latch=1\n'
        API['native_mode'](serial + event_text([good]) + receipt)
        for mutate in [lambda r:r['allocation'].__setitem__(1,r['allocation'][1]+4096),
                       lambda r:r['allocation'].__setitem__(1,r['allocation'][1]-4096),
                       lambda r:r['console'].__setitem__(1,r['console'][1]+4096),
                       lambda r:r['active'].__setitem__(0,r['active'][0]+16),
                       lambda r:r['armed'].__setitem__(0,r['armed'][0]+16),
                       lambda r:r['viewport_active'].__setitem__(3,1280|800<<16),
                       lambda r:r.update(firmware_writes=1)]:
            bad = copy.deepcopy(good); mutate(bad)
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                API['native_mode'](serial + event_text([bad]) + receipt)
        for bad in [receipt.replace('latch=1','latch=2'),
                    receipt.replace(str(good['requested']),str(good['requested']+4096)),
                    receipt.replace('firmware_writes=0','firmware_writes=1')]:
            with self.subTest(receipt=bad), self.assertRaises(ValueError):
                API['native_mode'](serial + event_text([good]) + bad)
        # The logical low surface has its own exact allocation; a larger EDID
        # bring-up allocation cannot serve as its extent evidence.
        low = console_event(2, 'mode', 0)
        API['display_memory'](serial + event_text([good, low]))
        low['allocation'][1] += 4096
        with self.assertRaises(ValueError):
            API['display_memory'](serial + event_text([good, low]))

    def test_bringup_allocation_sizes_stay_exact(self):
        good = MEMORY['evidence']()
        for index in range(len(good['objects'])):
            for delta in (-4096, 4096):
                bad = copy.deepcopy(good)
                bad['objects'][index]['size'] += delta
                serial = MEMORY['SERIAL'] + 'GT710_MEMORY ' + json.dumps(bad) + '\n'
                with self.subTest(object=index, delta=delta), self.assertRaises(ValueError):
                    API['display_memory'](serial)


if __name__ == '__main__':
    unittest.main()
