# /*
#  * Copyright 2012 Red Hat Inc.
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
# claim-lint:ok: quoted MIT grant retained in kernel/src/gpu/gk208/gr/channel.rs.
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
"""Exercise retained-channel CE startup with power-gated register reads.

MIT nouveau v6.8 nvkm/subdev/mc/base.c:nvkm_mc_enable uses the TOP reset
mask before engine access; nvkm/engine/ce/gk104.c defines CE status bits.
The TOP and PMC words below are from 20261028T084043Z-metal.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class DrawCeStart(unittest.TestCase):
    def test_power_gated_start_and_real_faults(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        method = item(source, '        fn prepare_copy(')
        state = item(source, '    struct DrawCopy {')
        harness = r'''
use std::collections::BTreeMap;
const ENABLE:u32=1<<21;
#[derive(Default)] struct Registers {
    values:BTreeMap<usize,u32>, accesses:Vec<(usize,u32)>, disabled_reads:usize,
}
impl Registers {
    fn read(&mut self,r:usize)->u32 {
        if (0x106000..0x107000).contains(&r) && self.values[&0x200]&ENABLE==0 {
            self.disabled_reads+=1;
            return 0xbadf1001;
        }
        *self.values.get(&r).unwrap_or(&0)
    }
    fn write(&mut self,r:usize,v:u32) {
        assert!(!(0x106000..0x107000).contains(&r) || self.values[&0x200]&ENABLE!=0,
                "write to disabled CE2");
        self.accesses.push((r,v));self.values.insert(r,v);
    }
    fn mask(&mut self,r:usize,m:u32,v:u32) {let old=self.read(r);self.write(r,(old&!m)|v);}
}
struct Hardware<'a> {regs:&'a mut Registers}
struct Identity {top:[u32;64],runlist:u32}
struct Report {identity:Identity}
STATE
struct Session {copy:Option<DrawCopy>,report:Report}
impl Session { METHOD }
fn run(enabled:bool,fault:u32) {
    let old=0xe011216d | if enabled {ENABLE} else {0};
    let mut regs=Registers::default();regs.values.insert(0x200,old);
    regs.values.insert(0x106904,0x12);regs.values.insert(0x106908,fault);
    let mut top=[0;64];top[..8].copy_from_slice(&[0x8006183e,3,0x90828c3e,7,0,0,0x9803aa3e,15]);
    let mut s=Session{copy:None,report:Report{identity:Identity{top,runlist:0}}};
    let result=s.prepare_copy(&mut Hardware{regs:&mut regs});
    assert_eq!(result.is_ok(),fault==0,"enabled={enabled}, fault={fault:#x}: {result:?}");
    assert_eq!(regs.disabled_reads,0,"status read before CE2 enabled");
    assert_eq!(regs.values[&0x200]&!ENABLE,old&!ENABLE,"another engine changed");
    assert!(!regs.accesses.iter().any(|&(r,_)|r==0x106908),"real CE faults were cleared");
    if fault==0 {
        let copy=s.copy.as_ref().unwrap();
        assert_eq!((copy.base,copy.engine,copy.reset),(0x106000,6,21));
        assert_eq!((copy.old_irq,copy.old_enable),(0x12,old&ENABLE));
        assert_eq!(regs.values[&0x106904],0);
        let count=regs.accesses.len();
        s.prepare_copy(&mut Hardware{regs:&mut regs}).unwrap();
        assert_eq!(regs.accesses.len(),count,"second transfer reinitializes CE");
    }
    // Execute the production CE restoration tail on either outcome.
    RESTORE
    assert_eq!(regs.values[&0x200],old,"PMC state leaked from probe");
    assert_eq!(regs.values[&0x106904],0x12,"CE interrupt mask changed");
}
fn main() {for enabled in [false,true] {for fault in [0,1,2,4] {run(enabled,fault);}}}
'''
        quiesce = item(source, '        pub(in crate::gk208::gr) fn quiesce(')
        restore = item(quiesce[quiesce.rindex('            if let Some(copy) = &self.copy {'):], '            if let Some(copy) = &self.copy {').replace('self.copy', 's.copy')
        harness = harness.replace('STATE', state).replace('METHOD', method).replace('RESTORE', restore)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'start.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'start.rs'),
                            '-o', str(path / 'start')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'start')], check=True)


if __name__ == '__main__':
    unittest.main()
