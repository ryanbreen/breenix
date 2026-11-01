# /*
#  * Copyright 2012 Red Hat Inc.
#  * Copyright 2013 Red Hat Inc.
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
# claim-lint:ok: MIT license condition; source paths in docs/gui/gt710-clock-record.md.
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
"""Fixed words exercise the production read-only GK104 clock/thermal decoder."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ClockDecoder(unittest.TestCase):
    def test_exact_source_vectors_and_refusals(self):
        source = r'''
extern crate alloc;
#[path="MODULE"] mod clock;
use std::collections::BTreeMap;
fn registers() -> BTreeMap<u32,u32> {
    BTreeMap::from([
        (0x101000,0x400000), // 27000 kHz crystal, device/base.c
        (0x137100,1), (0x137000,1), (0x137004,0x00021e01),
        (0x137120,0), (0x137140,0), (0x137250,0x80000200),
        (0x1373f4,2), (0x132000,1), (0x132004,0x10000201),
        (0x132020,1), (0x132024,0x00011401), (0x132030,0xf0000000),
        (0x137320,0), (0x137330,0),
        (0x22400,0x800), (0x21000,1), (0x212a8,1), (0x20400,47),
    ])
}
fn main() {
    let mut r=registers();
    let s=clock::sample(|a|*r.get(&a).unwrap_or(&0));
    assert_eq!(s.gpc_khz,Ok(202500)); // 27000*30/2 * 2/(2+2)
    assert_eq!(s.memory_khz,Ok(540000)); // 27000*20 * 2/2
    assert_eq!(s.host_khz,Ok(100000)); // gk104_clk_read href
    assert_eq!(s.temperature_c,Ok(47));
    assert!(s.raw.contains(&(0x212a8,1))); assert!(s.raw.contains(&(0x20400,47)));
    r.insert(0x132030,0); // fN=0 => 4096/8192 extra reference
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).memory_khz,Ok(553500));
    r.insert(0x132030,0xefff0000); // u16 wrapping fN+4096 = 65535
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).memory_khz,Ok(755996));
    r=registers();r.insert(0x137100,0);r.insert(0x137160,3);
    r.insert(0xe800,1);r.insert(0xe804,0x00003201);
    r.insert(0x1371d0,0x80000002);r.insert(0x137250,0x80000003);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Ok(270000));
    r.insert(0x137160,0x103);r.insert(0xe820,1);r.insert(0xe824,0x00002801);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Ok(216000));
    r.insert(0x137160,0x30000);r.insert(0x137250,0);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Ok(108000));
    r.insert(0x137160,2);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Ok(100000));
    r.insert(0x137160,1);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Err("unknown-divider-source"));
    r=registers();r.insert(0x1373f4,0);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).memory_khz,Err("unknown-memory-mode"));
    r=registers();r.insert(0x137004,0x21e00);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Err("zero-pll-divisor"));
    r=registers();r.insert(0x137000,0);
    assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Err("pll-disabled"));
    for strap in [0,0x40,0x400000,0x400040] {
        r=registers();r.insert(0x101000,strap);r.insert(0x137100,0);r.insert(0x137250,0x80000002);
        let expected=match strap {0=>13500,0x40=>14318,0x400000=>27000,_=>25000};
        assert_eq!(clock::sample(|a|*r.get(&a).unwrap_or(&0)).gpc_khz,Ok(expected*2/4));
    }
    for gate in [0x22400,0x21000] {
        r=registers();r.insert(gate,0);
        let s=clock::sample(|a|{assert!(a!=0x212a8 && a!=0x20400);*r.get(&a).unwrap_or(&0)});
        assert_eq!(s.temperature_c,Err("fuse-access-disabled"));
    }
    r=registers();r.insert(0x212a8,0);
    assert_eq!(clock::sample(|a|{assert_ne!(a,0x20400);*r.get(&a).unwrap_or(&0)}).temperature_c,Err("sensor-unavailable"));
}
'''.replace('MODULE', str(ROOT / 'kernel/src/gpu/gk208/clock.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp); (p / 'clock.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'clock.rs'), '-o', str(p / 'clock')], check=True)
            subprocess.run([str(p / 'clock')], check=True)


    def test_runtime_exports_two_read_only_records_accepted_by_scorer(self):
        import json
        from gt710_clock_report_test import API, anchor
        from gt710_vbios_perf_test import synthetic_rom
        source = r'''extern crate alloc;
#[path="SHA"] mod sha;
#[path="CLOCK"] mod clock;
fn hash(bytes:&[u8])->String { sha::sha256(bytes).iter().map(|v|format!("{v:02x}")).collect() }
fn main() {
    assert!(clock::export().is_none());
    let mut bar=vec![0u32;0x1000000/4];
    for (a,v) in [(0x101000,0x400000),(0x137100,1),(0x137000,1),(0x137004,0x21e01),
                  (0x137250,0x80000200),(0x1373f4,1),(0x132020,1),(0x132024,0x11401),
                  (0x132030,0xf0000000),(0x22400,0x800),(0x21000,1),(0x212a8,1),(0x20400,47)] {bar[a/4]=v;}
    let before=bar.clone();
    clock::initialize(bar.as_ptr() as usize);
    let rom=vec![ROM];let original=rom.clone();
    clock::initialize_performance(&rom);
    assert_eq!(rom,original);
    for _ in 0..2 {
        let (row,anchor)=clock::export().unwrap();println!("{}\n{}",row,anchor);
    }
    assert!(clock::export().is_none());
    assert_eq!(bar,before);
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            module = (ROOT/'kernel/src/gpu/gk208/clock.rs').read_text().replace('#[cfg(target_os = "none")]\n','')
            module = module.replace('path = \"perf.rs\"', 'path = \"'+str(ROOT/'kernel/src/gpu/gk208/perf.rs')+'\"')
            (p/'module.rs').write_text(module)
            source = source.replace('ROM',','.join(map(str,synthetic_rom())))
            source = source.replace('SHA',str(ROOT/'libs/libbreenix/src/crypto/sha256.rs')).replace('CLOCK',str(p/'module.rs'))
            (p/'runtime.rs').write_text(source)
            subprocess.run(priority + ['rustc','--edition=2021','-Dwarnings',str(p/'runtime.rs'),'-o',str(p/'runtime')],check=True)
            lines = subprocess.check_output([str(p/'runtime')],text=True).splitlines()
        self.assertEqual(len(lines),4)
        for phase, row, digest in [('startup',lines[0],lines[1]),('final',lines[2],lines[3])]:
            self.assertEqual(digest+'\n',anchor(row))
            data = API['clock_record'](json.loads(row.split(' ',1)[1]))
            self.assertEqual(data['phase'],phase)
            self.assertEqual(data['temperature_c']['value'],47)
            self.assertEqual(data['performance']['levels'][1]['core_khz'],405000)
            self.assertEqual(data['performance']['current'],dict(pstate=None,status='unmatched-clocks'))


if __name__ == '__main__': unittest.main()
