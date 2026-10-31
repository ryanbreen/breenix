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
#  * claim-lint:ok: MIT grant; pinned sources listed below.
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
# Layout: MIT nouveau nvkm/subdev/bios/{bit,perf}.c and clk/{base,gk104}.c
# at Linux v6.8 e8f897f4afef0031fe618a8e94127a0934896aba; headers checked.
"""Synthetic BIT P / performance-v40 decoding and clock-record evidence."""
import copy
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_clock_report_test import API, record, row

ROOT = Path(__file__).resolve().parents[1]


def synthetic_rom():
    rom = bytearray(512)
    rom[0x40:0x4c] = bytes([255, 184, 66, 73, 84, 0, 0, 1, 12, 6, 1, 0])
    rom[0x4b] = -sum(rom[0x40:0x4c]) & 255
    rom[0x4c:0x52] = bytes([80, 2, 4, 0, 0x60, 0])
    rom[0x60:0x64] = (0x100).to_bytes(4, 'little')
    # ver, header, base-entry length, subentry length, subentry count, levels.
    rom[0x100:0x106] = bytes([0x40, 6, 12, 4, 4, 3])
    for i, (pstate, gpc, mem) in enumerate([(15, 202, 540), (5, 810, 405), (0, 1908, 900)]):
        off = 0x106 + i * 28
        rom[off] = pstate
        for sub, mhz in enumerate([gpc, 777, 666, mem]):
            # Mask the high two bits; padding is deliberately nonzero.
            rom[off+12+sub*4:off+16+sub*4] = (0xc000 | mhz).to_bytes(2, 'little') + b'\xa5\x5a'
    return bytes(rom)


def performance():
    return dict(status='ok', bit_version=2, offset=256,
                raw=synthetic_rom()[256:346].hex(),
                levels=[dict(pstate=p, gpc_khz=g, core_khz=g//2, memory_khz=m)
                        for p,g,m in [(15,202000,540000),(5,810000,405000),(0,1908000,900000)]],
                current=dict(pstate=None, status='unmatched-clocks'))


def compile_run(source, test=False):
    priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
    with tempfile.TemporaryDirectory() as tmp:
        p = Path(tmp)
        (p/'test.rs').write_text(source)
        subprocess.run(priority + ['rustc','--edition=2021','-Dwarnings'] + (['--test'] if test else [])
                       + [str(p/'test.rs'),'-o',str(p/'test')],check=True)
        return subprocess.check_output([str(p/'test')],text=True)


class VbiosPerformance(unittest.TestCase):
    def test_synthetic_table_layout_matching_and_bounds(self):
        source = r'''extern crate alloc;
#[path="PERF"] mod perf;
fn rom()->Vec<u8> { vec![ROM] }
#[test] fn documented_v40_layout_and_current_level() {
    let r=rom(); let t=perf::decode(&r).unwrap();
    assert_eq!((t.bit_version,t.offset),(2,256));
    assert_eq!(t.raw,&r[256..346]);
    assert_eq!(t.levels.iter().map(|l|(l.pstate,l.gpc_khz,l.core_khz,l.memory_khz)).collect::<Vec<_>>(),
               vec![(15,202000,101000,540000),(5,810000,405000,405000),(0,1908000,954000,900000)]);
    assert_eq!(t.current(Ok(810000),Ok(405000)),(Some(5),"matched-clocks"));
    assert_eq!(t.current(Ok(810000),Ok(540000)),(None,"unmatched-clocks"));
    assert_eq!(t.current(Err("pll-disabled"),Ok(405000)),(None,"clock-unavailable"));
    let mut duplicate=r.clone();
    duplicate[302..318].copy_from_slice(&r[330..346]); // replace all four domains of level 5 with level 0
    let t=perf::decode(&duplicate).unwrap();
    assert_eq!(t.current(Ok(1908000),Ok(900000)),(None,"ambiguous-clocks"));
}
#[test] fn bounded_refusals_and_disabled_entries() {
    let r=rom();
    for n in 0..346 { assert!(perf::decode(&r[..n]).is_err(),"truncation {n}"); }
    for (off,value) in [(0x4d,3),(0x4e,3),(0x61,0),(0x62,1),(0x100,0x41),
                        (0x101,5),(0x102,0),(0x103,1),(0x104,3),(0x105,0)] {
        let mut bad=r.clone();bad[off]=value;
        assert!(perf::decode(&bad).is_err(),"offset {off:x}");
    }
    let mut disabled=r.clone();disabled[0x106]=255;
    let t=perf::decode(&disabled).unwrap();assert_eq!(t.levels.len(),2);
    assert_eq!(t.current(Ok(202000),Ok(540000)),(None,"unmatched-clocks"));
}
'''
        source = source.replace('PERF',str(ROOT/'kernel/src/gpu/gk208/perf.rs'))
        source = source.replace('ROM',','.join(map(str,synthetic_rom())))
        self.assertIn('2 passed',compile_run(source,True))

    def test_scorer_decodes_raw_table_and_refuses_forged_levels(self):
        data=record('startup'); data['performance']=performance()
        self.assertEqual(API['clock_record'](data),data)
        bads=[]
        for key in ('gpc_khz','core_khz','memory_khz','pstate'):
            bad=copy.deepcopy(data);bad['performance']['levels'][0][key]+=1;bads.append(bad)
        bad=copy.deepcopy(data);bad['performance']['current']=dict(pstate=5,status='matched-clocks');bads.append(bad)
        bad=copy.deepcopy(data);bad['performance']['raw']=bad['performance']['raw'][:-2];bads.append(bad)
        for bad in bads:
            with self.assertRaises(ValueError):API['clock_record'](bad)
        with contextlib.redirect_stdout(io.StringIO()) as output:
            API['clock_evidence'](row('startup',data)+row('final',dict(data,phase='final')))
        self.assertIn('pstate=5 core_khz=405000 memory_khz=405000',output.getvalue())
        self.assertIn('current=unmatched-clocks',output.getvalue())


if __name__ == '__main__': unittest.main()
