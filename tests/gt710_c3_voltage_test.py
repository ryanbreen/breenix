# /*
#  * Copyright 2012 Nouveau Community
#  * Copyright 2012 Red Hat Inc.
#  * Copyright 2013 Red Hat Inc.
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
# claim-lint:ok: MIT grant in docs/gui/gt710-c1.md.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  *
#  * Authors: Martin Peres
#  */
# claim-lint:ok: original MIT grant preserved under G1.
"""C3-0 check first: voltage lookup, VID sense and cstep/boost joins.

Synthetic inputs only. MIT nouveau layouts and arithmetic are cited in
docs/gui/gt710-c3-0.md; no captured board ROM is redistributed.
"""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/metal'))
import gt710_clock as clock
from gt710_c1_test import image


def board():
    d = clock.decode_image(image())
    d['voltage'].update(vidmask=31, min_uv=800000, max_uv=1187500,
                        entries=[dict(vid=i, uv=800000+12500*i) for i in range(32)])
    d['gpio']['entries'] = [dict(line=line, function=tag, log0=0, log1=1)
                            for line, tag in [(11, 4), (5, 5), (6, 6), (1, 0x1a), (0, 0x73)]]
    return d


def entry(mode, lo, hi, args, link=255):
    return dict(mode=mode, min_uv=lo, max_uv=hi, args=args+[0]*(6-len(args)), link=link)


class C3Voltage(unittest.TestCase):
    def test_gpio_vid_encoding_and_unmeasured_pins(self):
        d = board()
        raw = {0xd63c: 0x7000, 0xd624: 0x2000, 0xd628: 0x2000,
               0xd614: 0x7000, 0xd610: 0x2000}
        v = clock.voltage_state(d, raw)
        self.assertEqual((v['current_vid'], v['current_uv'], v['effective_vidmask']), (9, 912500, 31))
        self.assertEqual([(p['bit'], p['function'], p['line'], p['sense'], p['logical'])
                          for p in v['vid_pins']],
                         [(0, 4, 11, 1, 1), (1, 5, 5, 0, 0), (2, 6, 6, 0, 0),
                          (3, 0x1a, 1, 1, 1), (4, 0x73, 0, 0, 0)])
        # Active-low VID1: the same physical sense now contributes bit 1.
        d['gpio']['entries'][1].update(log0=1, log1=0)
        self.assertEqual(clock.voltage_state(d, raw)['current_vid'], 11)
        del raw[0xd628]
        self.assertIsNone(clock.voltage_state(d, raw)['current_vid'])
        raw[0xd628] = 0xbadf1201
        self.assertIsNone(clock.voltage_state(d, raw)['current_uv'])
        # Nouveau masks absent GPIO functions, but does not invent a read.
        d['gpio']['entries'].pop(2)
        self.assertEqual(clock.voltage_state(d, raw)['effective_vidmask'], 27)

    def test_vmap_temperature_speedo_rounding_and_links(self):
        d = board()
        d['voltage_map']['entries'] = [
            entry(0, 856250, 856250, [8528506, -3, 0], 1),
            entry(1, 0, 62500, [1048576, 0, -59]),
            entry(0, 800000, 1000000, [9000001, -11, 0]),
            entry(3, 900001, 925000, []),
            entry(2, 900001, 950000, []),
        ]
        q = clock.voltage_requirement(d, 0, temperature_c=32)
        self.assertEqual((q['min_uv'], q['max_uv'], q['requested_uv'], q['selected_uv'], q['selected_vid']),
                         (889941, 889941, 889941, 900000, 8))
        self.assertEqual(q['missing'], [])  # Fixed clamp and zero speedo coefficients.
        q = clock.voltage_requirement(d, 0)
        self.assertEqual((q['min_uv'], q['max_uv'], q['missing']),
                         (856250, 918750, ['temperature_c']))
        q = clock.voltage_requirement(d, 2)
        self.assertIsNone(q['requested_uv'])
        self.assertEqual(q['missing'], ['speedo'])
        q = clock.voltage_requirement(d, 2, speedo=1)
        self.assertEqual((q['requested_uv'], q['selected_uv']), (899999, 900000))
        self.assertEqual(clock.voltage_requirement(d, 3)['requested_uv'], 912500)
        self.assertEqual(clock.voltage_requirement(d, 4)['selected_uv'], 912500)
        d['voltage_map']['entries'][0]['link'] = 0
        with self.assertRaisesRegex(ValueError, 'cyclic voltage map'):
            clock.voltage_requirement(d, 0, temperature_c=32)

    def test_cstep_boost_join_base_floor_and_unknown_maximum(self):
        d = board()
        d['performance']['levels'] = [dict(pstate=15, voltage_id=0, gpc_khz=1307000,
                                            core_khz=653500, memory_khz=900000)]
        d['voltage_map']['entries'] = [entry(2, 890000, 890000, []),
                                      entry(2, 850000, 850000, []),
                                      entry(0, 900000, 950000, [9000000, 10, 0]),
                                      entry(2, 925000, 925000, [])]
        d['cstep'].update(levels=[dict(pstate=15, index=3)], points=[
            dict(frequency_khz=f, voltage_id=v, unknown=[0, 1])
            for f, v in [(810000, 1), (1307000, 1), (1516000, 2), (1647000, 3), (0, 0)]])
        d['boost']['levels'] = [dict(pstate=15, min_khz=1307000, max_khz=1647000,
                                     domains=[dict(domain=0, percent=100, min_khz=1307000, max_khz=1647000)])]
        raw = {0xd63c: 0x4000, 0xd624: 0, 0xd628: 0, 0xd614: 0x4000, 0xd610: 0}
        v = clock.voltage_state(d, raw)
        points = v['cstep_levels'][0]['points']
        self.assertEqual([(p['index'], p['raw_gpc_khz'], p['gpc_khz'], p['memory_khz']) for p in points],
                         [(0, 810000, 1307000, 900000), (1, 1307000, 1307000, 900000),
                          (2, 1516000, 1516000, 900000), (3, 1647000, 1647000, 900000)])
        self.assertEqual(points[0]['requirement']['selected_uv'], 900000)  # Pstate base floor.
        self.assertEqual(v['highest_at_current_uv'], dict(guaranteed_gpc_khz=1307000,
                                                        possible_gpc_khz=1516000, exact=False))
        exact = clock.voltage_state(d, raw, speedo=100)
        self.assertEqual(exact['highest_at_current_uv'], dict(guaranteed_gpc_khz=1516000,
                                                            possible_gpc_khz=1516000, exact=True))
        # A boost record carries constraints, not its own voltage or memory target.
        self.assertEqual(v['boost_levels'][0]['memory_khz'], 900000)
        self.assertEqual(v['boost_levels'][0]['voltage_source'], 'cstep-and-performance')
        self.assertEqual(v['unused_cstep_indices'], [4])


if __name__ == '__main__':
    unittest.main()
