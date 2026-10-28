# /*
#  * Copyright 2012 Red Hat Inc.
#  * Copyright 2021 Red Hat Inc.
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
# claim-lint:ok: quoted MIT grant also in kernel/src/gpu/gk208/ce/recovery.rs.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  */
"""Execute the production transport reset and check preempt/reset ordering.

The native engine must stop accepting the old PBDMA context before its reset;
preempting after reset can save pre-reset state into the reinitialized engine.
Reference: MIT nouveau fifo/runl.c:nvkm_runl_rc_runl/nvkm_runl_rc and
fifo/gk104.c:gk104_chan_stop at Linux v6.8 e8f897f4afef0031fe618a8e94127a0934896aba.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class ResetOrder(unittest.TestCase):
    def test_preempt_is_requested_before_the_active_ce_is_reset(self):
        source = (ROOT / 'kernel/src/gpu/gk208/ce/recovery.rs').read_text()
        begin = source.index('    regs.mask(0x2630', source.index('    let old_eio = ['))
        end = source.index('    let reset_pending', begin)
        block = source[begin:end]
        wait = source[source.index('fn budget_wait('):source.index('/// Stop scheduling A.')].strip()
        harness = r'''
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64,AtomicU32,Ordering};
static CLOCK:AtomicU64=AtomicU64::new(0);
static RESETS:AtomicU32=AtomicU32::new(0);
fn elapsed(start:u64)->u64 {CLOCK.load(Ordering::SeqCst)-start}
#[derive(Default)] struct Registers { values: BTreeMap<usize,u32>, writes: Vec<(usize,u32)>, pending:u32, stuck:bool }
impl Registers {
    fn read(&mut self, r: usize) -> u32 {
        if r==0x2634 && self.pending!=0 {
            if !self.stuck {self.pending-=1;}
            if self.pending!=0 {return 0x100000;}
        }
        *self.values.get(&r).unwrap_or(&0)
    }
    fn write(&mut self, r: usize, v: u32) {
        if r==0x2634 {self.pending=3;}
        if r==0x200 && v & 64==0 {
            assert_eq!(self.pending,0,"CE reset raced unfinished preemption");
            RESETS.fetch_add(1,Ordering::SeqCst);
        }
        self.writes.push((r,v)); self.values.insert(r,v);
    }
    fn mask(&mut self, r: usize, m: u32, v: u32) { let old=self.read(r);self.write(r,(old & !m)|v); }
    fn delay_us(&mut self, us: u64) {CLOCK.fetch_add(us,Ordering::SeqCst);}
}
struct Engine {run:u32,reset:u32,base:usize,pbdma:usize,engine:usize}
struct Range {start:u64}
struct Allocation {range:Range}
struct Channel {id:u32,commands:Allocation}
const RUNLIST:u64=4096;
WAIT
fn transport_confirmed(_: &mut Registers,_:&Engine,_:u32,_:u32)->Result<(), &'static str> {Ok(())}
fn run(stuck:bool)->Result<(), &'static str> {
    let mut regs=Registers{stuck,..Registers::default()};regs.values.insert(0x200,0x1000140);
    let e=Engine{run:4,reset:6,base:0x104000,pbdma:0,engine:4};
    let a=Channel{id:0,commands:Allocation{range:Range{start:0x100000}}};
    let b=Channel{id:1,commands:Allocation{range:Range{start:0x200000}}};
    assert_eq!(a.id,0); assert_eq!(b.id,1);assert_eq!(b.commands.range.start,0x200000);
    let reset_start=0;
BLOCK
    let disable=regs.writes.iter().position(|(r,v)| *r==0x200 && v & 64==0).unwrap();
    let preempt=regs.writes.iter().position(|(r,v)| *r==0x2634 && *v==b.id).expect("active channel was not preempted");
    assert!(preempt < disable,"CE reset happened before the active PBDMA channel preempt request");
    assert_eq!(regs.read(0x200)&!64,0x1000100);
    Ok(())
}
fn main() {
    run(false).unwrap();
    RESETS.store(0,Ordering::SeqCst);CLOCK.store(0,Ordering::SeqCst);
    assert_eq!(run(true),Err("native reset budget exhausted; backing retained"));
    assert_eq!(RESETS.load(Ordering::SeqCst),0,"timed-out preempt still reset CE");
    assert_eq!(CLOCK.load(Ordering::SeqCst),100_000);
}
'''.replace('BLOCK', block).replace('WAIT', wait)
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory);(path/'reset.rs').write_text(harness)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(path/'reset.rs'),'-o',str(path/'reset')],check=True,cwd=ROOT)
            subprocess.run([str(path/'reset')],check=True)

if __name__=='__main__':unittest.main()
