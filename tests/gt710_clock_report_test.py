# /*
#  * Copyright 2012, 2013 Red Hat Inc.
#  * Copyright 2014 Martin Peres
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
# Adapted vectors from MIT nouveau clk/gk104.c, therm/g84.c, fuse/gf100.c
# and engine/device/base.c at Linux v6.8; see THIRD_PARTY_NOTICES.
"""Clock records are authenticated, slot-bound, required and printed by 8.7."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from gt710_resources_report_test import API, ROOT


def record(phase):
    raw = [[0x101000,0x400000], [0x137100,1], [0x137000,1],
           [0x137004,0x21e01], [0x137120,0], [0x137140,0], [0x137250,0x80000200],
           [0x1373f4,2], [0x132000,1], [0x132004,0x10000201],
           [0x132020,1], [0x132024,0x11401], [0x132030,0xf0000000],
           [0x137320,0], [0x137330,0], [0x22400,0x800], [0x21000,1],
           [0x212a8,1], [0x20400,47]]
    return dict(schema=1, phase=phase, host_source='href', raw=raw,
                gpc_khz=dict(value=202500, status='ok'), memory_khz=dict(value=540000, status='ok'),
                host_khz=dict(value=100000, status='ok'), temperature_c=dict(value=47, status='ok'))


def row(phase, data=None):
    return 'GT710_CLOCK_V1 ' + json.dumps(data or record(phase), separators=(',', ':')) + '\n'


def anchor(text):
    data = json.loads(text.split(' ',1)[1])
    raw = text.rstrip('\n').encode()
    return f'GT710_CLOCK_BULK phase={data["phase"]} bytes={len(raw)} sha256={hashlib.sha256(raw).hexdigest()}\n'


def bulk_anchor(prefix, text):
    raw = text.encode()
    return f'{prefix} {len(raw)} {hashlib.sha256(raw).hexdigest()}\n'


class ClockReport(unittest.TestCase):
    def test_both_records_are_required_and_printed_with_performance(self):
        from gt710_performance_report_test import PerformanceReport, fixture
        text = fixture()
        output = PerformanceReport().score(text)
        for phase in ('startup', 'final'):
            self.assertIn(f'{phase} 202500 540000 100000 47', output)
            with self.assertRaises(ValueError):
                PerformanceReport().score(text.replace(row(phase), ''))
        self.assertIn('host_reference_khz', output)
        self.assertIn('performance/clear-rate-2560 clears-per-second=60', output)
        with self.assertRaises(ValueError):
            API['clock_evidence'](row('startup') + row('startup'))
        bad = record('final'); bad['memory_khz']['value'] += 1
        with self.assertRaises(ValueError):
            API['clock_evidence'](row('startup') + row('final', bad))
        unavailable = record('final')
        unavailable['raw'] = [r for r in unavailable['raw'] if r[0] not in (0x212a8,0x20400)]
        next(r for r in unavailable['raw'] if r[0] == 0x22400)[1] = 0
        unavailable['temperature_c'] = dict(value=None, status='fuse-access-disabled')
        with contextlib.redirect_stdout(io.StringIO()) as out:
            API['clock_evidence'](row('startup') + row('final', unavailable))
        self.assertIn('unavailable:fuse-access-disabled', out.getvalue())

    def test_slots_anchors_and_raw_decoding_are_checked(self):
        first, last = row('startup'), row('final')
        middle = ('SUITE gpu START cases=170\nSUITE gpu CASE performance/present-rate-2560 PASS ms=1\n'
                  'SUITE gpu DONE passed=170 failed=0 skipped=0 total=170\n'
                  'SUITE gpu MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n')
        serial = anchor(first) + bulk_anchor('GT710_BULK', first) + middle + anchor(last) + bulk_anchor('GT710_FINAL_BULK', last) + 'SUITE_SEQUENCE READY gpu\n'
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            def stage(s=serial, a=first, b=last):
                (p/'serial.txt').write_text(s);(p/'gt710-pixels.rle').write_text(a);(p/'gt710-final.rle').write_text(b)
            stage()
            self.assertIn(first + anchor(first), API['capture_serial'](p))
            self.assertIn(last + anchor(last), API['capture_serial'](p))
            for s,a,b in [(serial.replace(anchor(last), ''),first,last),
                          (serial.replace(anchor(last),anchor(last)*2),first,last),
                          (serial,first,last.replace('540000','540001')),
                          (serial.replace(anchor(first),first+anchor(first)),first,last),
                          (serial.replace(bulk_anchor('GT710_BULK',first),bulk_anchor('GT710_BULK',last)).replace(bulk_anchor('GT710_FINAL_BULK',last),bulk_anchor('GT710_FINAL_BULK',first)),last,first)]:
                stage(s,a,b)
                with self.assertRaises(ValueError): API['capture_serial'](p)


if __name__ == '__main__': unittest.main()
