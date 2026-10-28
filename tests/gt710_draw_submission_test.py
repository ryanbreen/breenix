# /*******************************************************************************
#     Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.
# 
#     Permission is hereby granted, free of charge, to any person obtaining a
#     copy of this software and associated documentation files (the "Software"),
#     to deal in the Software without restriction, including without limitation
#     the rights to use, copy, modify, merge, publish, distribute, sublicense,
#     and/or sell copies of the Software, and to permit persons to whom the
#     Software is furnished to do so, subject to the following conditions:
# 
#     The above copyright notice and this permission notice shall be included in
#     all copies or substantial portions of the Software.
# claim-lint:ok: quoted MIT grant retained in kernel/src/gpu/gk208/gr/channel.rs.
# 
#     THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#     IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#     FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#     THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
#     LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
#     FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
#     DEALINGS IN THE SOFTWARE.
# 
# *******************************************************************************/
"""Run production CE/draw submission wrappers through the consumption boundary.

NVIDIA open-gpu-doc 9fdf5c4 classes/host/cla06f.h:125-126 (MIT):
SET_REFERENCE is method 0x50, with the full 32-bit count. A06F DMA_INCR
encodes its one-word packet as 0x20010014. The engine receipt stays separate.
Reproduces 090139Z: GET advanced to 6 while reference remained at 5.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class DrawSubmission(unittest.TestCase):
    def test_copy_and_draw_publish_their_submission_reference(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        methods = '\n'.join(item(source, marker) for marker in [
            '        fn sequence(', '        pub(in crate::gk208::gr) fn copy_rect(',
            '        pub(in crate::gk208::gr) fn draw('])
        methods = methods.replace('pub(in crate::gk208::gr)', 'pub(crate)')
        methods = methods.replace('crate::gk208::copy', 'crate::copy').replace('super::super::', 'crate::')
        harness = r'''
extern crate alloc;
#[path="ROOT/kernel/src/gpu/gk208/gr/channel.rs"] pub mod channel;
#[path="ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] pub mod shaders;
#[path="ROOT/kernel/src/gpu/gk208/gr/draw.rs"] pub mod draw;
#[path="ROOT/kernel/src/gpu/gk208/gr/binding.rs"] pub mod binding;
#[path="ROOT/kernel/src/gpu/gk208/copy.rs"] pub mod copy;
use channel::{Observation,receipt_nonce};
const BOUND_US:u64=2_000_000;
const TARGET:u64=0x600000;
struct Registers;
impl Registers {fn read(&self,_:usize)->u32 {0}}
struct Hardware<'a> {regs:&'a mut Registers}
struct Identity {epoch:u64}
struct Report {identity:Identity}
struct Session {report:Report,next_sequence:u32,prepared:bool,accept:bool,stream:Vec<u32>}
impl Session {
    fn prepare_copy(&mut self,_:&mut Hardware<'_>)->Result<(),&'static str> {self.prepared=true;Ok(())}
    fn issue_words(&mut self,_:&mut Hardware<'_>,seq:u32,poison:u32,nonce:u32,budget:u64,
                   stream:&[u32],stop_on_trap:bool)->Result<Observation,&'static str> {
        assert_eq!((poison,budget,stop_on_trap),(85,BOUND_US,true));
        assert_eq!(&stream[stream.len()-2..], &[0x20010014,seq],
                   "GET can advance while the host reference stays at the preceding submission");
        self.stream=stream.to_vec();
        Ok(Observation{value:nonce,epoch:self.report.identity.epoch,consumed:true,
                       elapsed_us:10,accepted:self.accept})
    }
METHODS
}
fn session(seq:u32,accept:bool)->Session {
    Session{report:Report{identity:Identity{epoch:0x123456789abcdef0}},next_sequence:seq,
            prepared:false,accept,stream:vec![]}
}
#[test] fn copy_reference_follows_engine_release() {
    let rect=copy::Rect{src:0x100000,dst:0x200000,width:48,height:1,src_pitch:48,dst_pitch:48};
    for seq in [6,37,511] {for accept in [false,true] {
        let mut s=session(seq,accept);
        let result=s.copy_rect(&mut Hardware{regs:&mut Registers},rect);
        assert_eq!(result,if accept {Ok(())} else {Err("draw CE release not observed")});
        assert!(s.prepared);
        let nonce=receipt_nonce(s.report.identity.epoch,true).wrapping_add(seq);
        let expected=copy::encode(rect,TARGET,s.report.identity.epoch,nonce).unwrap();
        assert_eq!(&s.stream[..s.stream.len()-2],expected,"CE epoch/nonce release changed");
        assert_eq!(s.next_sequence,seq+1);
    }}
}
#[test] fn draw_reference_follows_engine_release() {
    for seq in [7,38,511] {for accept in [false,true] {
        let mut s=session(seq,accept);
        let request=draw::Draw{target_va:0x100000,pitch:256,width:64,height:64,vertex_va:0x200000,
                              vertex_program:0x30,fragment_program:0x1b0,report_va:0,epoch:0,nonce:0};
        let mut receipt=binding::Receipt{target:0,epoch:0,nonce:0,before:[0;4],after:[0;4],observation:None};
        let result=s.draw(&mut Hardware{regs:&mut Registers},request,&mut receipt);
        assert_eq!(result,if accept {Ok(())} else {Err("draw engine release not observed")});
        let nonce=receipt_nonce(s.report.identity.epoch,true).wrapping_add(seq);
        let expected=draw::commands(draw::Draw{report_va:TARGET,epoch:s.report.identity.epoch,nonce,..request}).unwrap();
        assert_eq!(&s.stream[..s.stream.len()-2],expected,"A197 draw/engine release changed");
        assert_eq!((receipt.target,receipt.epoch,receipt.nonce),(TARGET,s.report.identity.epoch,nonce));
        assert_eq!(receipt.observation.unwrap().accepted,accept);
        assert_eq!(s.next_sequence,seq+1);
    }}
}
'''.replace('ROOT', str(ROOT)).replace('METHODS', methods)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'submission.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path / 'submission.rs'), '-o', str(path / 'submission')], check=True)
            subprocess.run([str(path / 'submission'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
