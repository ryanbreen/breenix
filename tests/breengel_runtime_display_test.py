"""G27 BreenGL display dispatch keeps G20 GPU coverage and exact latch checks."""
from pathlib import Path
import runpy
import unittest

ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/screen-proof-acceptance.py'))
FIXTURES=runpy.run_path(str(ROOT/'tests/gt710_display_evidence_test.py'))
NATIVE=FIXTURES['API']
row=FIXTURES['row']
text=FIXTURES['text']

class BreenGLRuntimeDisplay(unittest.TestCase):
    def check(self, rows, suite='breengel'):
        native=dict(NATIVE)
        native['board_inventory']=lambda serial: {}
        native['native_raster']=lambda board: dict(width=1280,height=1024)
        check = API['check_breengel_runtime_display'] if suite=='breengel' else API['check_runtime_display']
        return check(text(rows),native)

    def test_g27_mode_one_is_accepted_and_other_inventories_refused(self):
        self.assertTrue(self.check([row(1)]))
        for rows in [[],[row(0)],[row(0),row(1)]]:
            with self.subTest(rows=rows),self.assertRaises(ValueError):
                self.check(rows)
        with self.assertRaisesRegex(ValueError,'evidence storage failed'):
            API['check_breengel_runtime_display']('GT710_DISPLAY_EVIDENCE_ERROR\n'+text([row(1)]),{})

    def test_gpu_dispatch_still_requires_both_g20_modes(self):
        self.assertTrue(self.check([row(0),row(1)],'gpu'))
        for rows in [[row(0)],[row(1)],[]]:
            with self.subTest(rows=rows),self.assertRaises(ValueError):
                self.check(rows,'gpu')

    def test_g27_keeps_the_existing_exact_latch_and_storage_refusals(self):
        for change in [lambda r:r.update(requested=r['requested']+4096),
                       lambda r:r['armed'].__setitem__(0,r['armed'][0]+1),
                       lambda r:r['viewport_active'].__setitem__(5,1280|1024<<16),
                       lambda r:r['raster'].__setitem__(1,1639|840<<16),
                       lambda r:r.update(firmware_writes=1),
                       lambda r:r.update(notifier=0)]:
            bad=row(1);change(bad)
            with self.subTest(change=change),self.assertRaises(ValueError):
                self.check([bad])

if __name__=='__main__':unittest.main()
