"""G25 fallback: the DDC preferred raster keeps native evidence exact."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
NATIVE = runpy.run_path(str(ROOT/'tests/gt710_native_report_test.py'))
G21 = runpy.run_path(str(ROOT/'tests/gt710_g21_receipt_test.py'))
RAW = bytes.fromhex((ROOT/'tests/fixtures/gt710-edid/lcd17-19.hex').read_text())
DTD = dict(width=1280,height=1024,clock_khz=108000,hsync_start=1328,
           hsync_end=1440,htotal=1688,vsync_start=1025,vsync_end=1028,vtotal=1066,flags=30)
LOW = dict(width=1024,height=768,clock_khz=65000,hsync_start=1048,
           hsync_end=1184,htotal=1344,vsync_start=771,vsync_end=777,vtotal=806,flags=24)
DMT75 = dict(DTD,clock_khz=135000,hsync_start=1296)


def capture(fast=True, advertise=True):
    raw = bytearray(RAW)
    if not advertise:
        raw[36] &= ~1
        raw[127] = 0
        raw[127] = -sum(raw) & 255
    m = NATIVE['capture'].__globals__['M']
    board = dict(m['board'],edid=raw.hex(),edid_sha256=hashlib.sha256(raw).hexdigest(),
                 modes=[DTD,LOW]+([DMT75] if advertise else []))
    pci = runpy.run_path(str(ROOT/'tests/gt710_report_test.py'))['SERIAL']
    evidence = m['evidence']
    def timing(e):
        e['timing'] = DMT75 if fast else DTD
        e['pll_coefficients'] = 0x074502 if fast else 0x094702
        e['pll_fraction'] = 0x10000000
        values = {0x414:1066<<16|1688,0x418:2<<16|(143 if fast else 111),
                  0x41c:40<<16|(391 if fast else 359),0x420:1064<<16|(1671 if fast else 1639),
                  0x450:135000000 if fast else 108000000}
        for r in e['active']:
            r[1] = values.get(r[0]-0x640000,r[1])
    with patch.dict(m,board=board,SERIAL=pci+'GT710_NATIVE '+json.dumps(board)+'\n',
                    evidence=lambda:evidence(1280,1024)):
        return NATIVE['capture'](timing)


def mutate_record(serial, prefix, change):
    line = next(l for l in serial.splitlines() if l.startswith(prefix))
    e = json.loads(line[len(prefix):]); change(e)
    return serial.replace(line,prefix+json.dumps(e))


class RasterEvidence(unittest.TestCase):
    def test_captured_edid_decodes_established_dmt_without_replacing_preferred_dtd(self):
        self.assertEqual(hashlib.sha256(RAW).hexdigest(),
                         '07da84b6557390f56c0b274dd93cc9a6e09f14684bc97b87e9978cdb0d86ff2c')
        self.assertEqual(API['edid_timings'](RAW),[DTD,LOW,DMT75])

    def test_selected_native_raster_and_fallback_keep_full_pixels_pll_and_g21_receipts(self):
        preferred = capture(fast=False)
        mode, pixels = API['native_mode'](preferred)
        self.assertEqual(mode['timing'],DTD)
        self.assertEqual(len(pixels),1280*1024*4)
        API['native_mode'](capture(fast=False,advertise=False))
        # A coherent 75 Hz MODE/PLL/register set fails even when advertised.
        for bad in [capture(),capture(advertise=False),
                    mutate_record(preferred,'GT710_MODE ',lambda r:r.update(pll_coefficients=0x074502)),
                    mutate_record(preferred,'GT710_MODE ',lambda r:r['active'][1].__setitem__(1,2<<16|143)),
                    mutate_record(preferred,'GT710_MODE ',lambda r:r.update(notifier_after=0)),
                    preferred.replace('GT710_PIX 0 ','GT710_PIX 1 ',1)]:
            with self.subTest(rejection=bad[-200:]),self.assertRaises(ValueError):
                API['native_mode'](bad)
        # G20 surfaces and G21 allocation/latch joins stay exact at the preferred raster.
        rows = [G21['console_event'](),G21['console_event'](2,'receipt')]
        for r in rows:
            r['raster'] = [359|40<<16,1639|1064<<16]
        receipt = f'GT710_NATIVE_CONSOLE surface={rows[1]["requested"]} firmware_writes=0 latch=2\n'
        logical = preferred+'GT710_NATIVE_HANDOFF latch=1\n'+G21['event_text'](rows)+receipt
        API['native_mode'](logical)
        bad = copy.deepcopy(rows); bad[1]['allocation'][1] += 4096
        with self.assertRaises(ValueError):
            API['native_mode'](preferred+'GT710_NATIVE_HANDOFF latch=1\n'+G21['event_text'](bad)+receipt)


if __name__ == '__main__':
    unittest.main()
