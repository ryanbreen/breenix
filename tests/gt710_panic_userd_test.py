"""A PRAMIN storage write does not notify the GPU's BAR1 USERD aperture."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]

class UserdPublication(unittest.TestCase):
    def test_terminal_packets_require_bar1_publication(self):
        source = (ROOT/'kernel/src/gpu/gk208/emergency.rs').read_text()
        body = source[source.index('struct Terminal('):source.index('/// Preparation only: map immutable')]
        for start,end in [('    /// Preparation only: share','    pub(crate) fn packet('),
                          ('    pub(crate) fn prepare(','    pub(crate) fn bound(')]:
            a=body.index(start);b=body.index(end,a);body=body[:a]+body[b:]
        fixture=(ROOT/'tests/fixtures/gt710_panic_transport.rs').read_text()
        # Keep the existing receipt, ring-wrap, timeout and display assertions.
        # Only the mock device changes: BAR1 publication, not a VRAM store,
        # advances the device and writes an engine receipt.
        fixture=fixture.replace('withheld_flip:bool }','withheld_flip:bool, bar:Vec<u32> }')
        fixture=fixture.replace('if a==0x10008c {','if a==0x10008c && false {')
        a=fixture.index('    let q=Queue{')
        fixture=fixture[:a]+'    let mut bar=vec![0u32;128];\n'+fixture[a:]
        # Bind the real transport snapshot's extra alias when it exists. The
        # old transport still compiles and fails at execution, not at setup.
        if 'pub userd_bar: usize' in body:
            fixture=fixture.replace('ring:0x200000,userd:0x100000,',
                'ring:0x200000,userd:0x100000,userd_bar:bar.as_mut_ptr() as usize,')
        else:
            fixture=fixture.replace('    let mut bar=vec![0u32;128];','    let bar=vec![0u32;128];')
        fixture=fixture.replace('*d=Device::default();','*d=Device::default();d.bar=bar;')
        fixture=fixture.replace('d.vram.insert(q.userd+0x8c,511);',
            'd.vram.insert(q.userd+0x8c,511);d.bar[0x8c/4]=511;')
        fixture+='''
mod gpu_cache {
 pub fn drain_wc() { crate::DEVICE.with(|d| {
  let mut d=d.borrow_mut(); let put=d.bar[0x8c/4];
  d.vram.insert(0x10008c,put);d.vram.insert(0x100088,put);
  if !d.withheld { unsafe {
   (d.release as *mut u32).write_volatile(d.nonce);
   ((d.release+4) as *mut u32).write_volatile(d.epoch as u32);
   ((d.release+8) as *mut u32).write_volatile((d.epoch>>32) as u32);
  }}
 }); }
}
'''
        # The old implementation never drains a BAR1 publication; avoid a
        # fixture-only dead_code warning obscuring the intended runtime RED.
        fixture=fixture.replace('fn complete_and_wrapping_receipts() {',
          'fn complete_and_wrapping_receipts() {\n    let _:fn()=gpu_cache::drain_wc;')
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'test.rs').write_text(fixture.replace('// TRANSPORT',body))
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','--test','--cfg','feature="capture_panic_oracle"','-Dwarnings',str(p/'test.rs'),'-o',str(p/'test')],check=True)
            subprocess.run([str(p/'test'),'--test-threads=1'],check=True)
if __name__=='__main__':unittest.main()
