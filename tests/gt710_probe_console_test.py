"""Execute the real inventory handoff tail with a refused GR probe.

Only the hardware calls are substituted. The production control flow is compiled
unchanged, so propagating the GR error loses the console and fails this test.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ProbeConsole(unittest.TestCase):
    def test_gr_refusal_returns_native_surface_and_retains_its_backing(self):
        source = (ROOT / 'kernel/src/gpu/gk208/mod.rs').read_text()
        start = source.index('        if native::suite_requested() {')
        end = source.index('        return Ok(console);', start) + len('        return Ok(console);')
        tail = source[start:end]
        harness = r'''
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
static SNAPSHOTS: AtomicU32 = AtomicU32::new(0);
static PMC: AtomicU32 = AtomicU32::new(0);
static PMC_WRITES: AtomicU32 = AtomicU32::new(0);
static REFUSE: AtomicBool = AtomicBool::new(false);
static RETAINED: AtomicBool = AtomicBool::new(false);
static REPORTED: AtomicBool = AtomicBool::new(false);
#[macro_export]
macro_rules! serial_println {
    ($($a:tt)*) => {{
        let line = format!($($a)*);
        if line.contains("GR probe refused") { REPORTED.store(true, Ordering::SeqCst); }
    }};
}
#[derive(Clone, Copy, Debug, PartialEq)] struct Console { pointer: usize }
struct Range { start: u64, size: u64 }
struct Object { range: Range }
struct Layout { objects: [Object; 1] }
struct Registers(usize, u32, u32);
impl Registers {
    fn read(&self, address: usize) -> u32 {
        match address {
            0x200 => PMC.load(Ordering::SeqCst),
            0x800000 => self.1,
            0x409b00 => self.2,
            _ => panic!("unexpected register read {address:#x}"),
        }
    }
    fn mask(&mut self,address: usize,mask:u32,value:u32) {
        assert_eq!((address,mask),(0x200,0x1000));
        let before=self.read(address);PMC.store((before & !mask)|value,Ordering::SeqCst);
        PMC_WRITES.fetch_add(1,Ordering::SeqCst);
    }
}
struct Bar { size: u64 }
struct Device { bars: [Bar; 2] }
mod timer { pub fn rdtsc() -> u64 { 1 } }
mod native { pub fn suite_requested() -> bool { true } }
mod channel {
    use super::*;
    pub fn elapsed(_: u64) -> u64 { 1 }
    pub fn probe(_: &mut Registers, _: &Device, _: &mut Layout, _: &Console) -> Result<(), &'static str> { Ok(()) }
}
mod ce {
    use super::*;
    pub fn probe(_: &mut Registers, _: &Device, _: &mut Layout, _: &Console) -> Result<(), &'static str> { Ok(()) }
    pub fn retain(layout: Layout, regs: usize, bar: usize) -> Result<(), &'static str> {
        assert_eq!((layout.objects[0].range.start, regs, bar), (4096, 8192, 16384));
        RETAINED.store(true, Ordering::SeqCst); Ok(())
    }
}
mod gr {
    use super::*;
    pub fn interrupt_snapshot(_: &mut Registers, _: &Device, phase: &str) {
        SNAPSHOTS.fetch_or(match phase {"before"=>1,"after"=>2,_=>panic!("unknown snapshot phase")},Ordering::SeqCst);
    }
    pub fn probe(regs: &mut Registers, _: &Device, _: &mut Layout, _: &Console, _: &mut ()) -> Result<(), &'static str> {
        if REFUSE.load(Ordering::SeqCst) { regs.mask(0x200,0x1000,0x1000); Err("GR probe refused") } else { Ok(()) }
    }
}
mod modeset {
    pub fn bar1_view(_: u64, _: u64, _: u64, _: u8) -> Result<(u64, usize, u64), &'static str> { Ok((0, 256, 0)) }
}
fn handoff(mut regs: Registers, device: Device, mut layout: Layout, console: Console) -> Result<Console, &'static str> {
let mut evo=();
TAIL
}
#[test]
fn clean_and_refused_gr_both_deliver_the_native_console() {
    for initial in [0xe011216d,0xe011316d] {
    for refuse in [false, true] {
    for binding in [0,0x80000300] {
    for current in [0,0x80123456] {
        SNAPSHOTS.store(0,Ordering::SeqCst);
        PMC.store(initial,Ordering::SeqCst);
        PMC_WRITES.store(0,Ordering::SeqCst);
        REFUSE.store(refuse, Ordering::SeqCst);
        RETAINED.store(false, Ordering::SeqCst);
        REPORTED.store(false, Ordering::SeqCst);
        let console = Console { pointer: 16640 };
        let result = handoff(Registers(8192,binding,current), Device { bars: [Bar {size: 0}, Bar {size: 1 << 27}] },
            Layout { objects: [Object {range: Range {start: 4096, size: 4096}}] }, console);
        assert_eq!(SNAPSHOTS.load(Ordering::SeqCst),3);
        assert_eq!(result, Ok(console), "GR refusal detached the screen console");
        if refuse && (binding != 0 || current != 0) {
            assert_eq!(PMC.load(Ordering::SeqCst),initial|0x1000,"live GR context was power cycled");
            assert_eq!(PMC_WRITES.load(Ordering::SeqCst),1,"outer restore wrote PMC with live GR state");
        } else {
            assert_eq!(PMC.load(Ordering::SeqCst),initial,"GR refusal leaked its PMC enable into CE recovery");
            assert_eq!(PMC_WRITES.load(Ordering::SeqCst),if refuse {2} else {0});
        }
        assert!(RETAINED.load(Ordering::SeqCst));
        assert_eq!(REPORTED.load(Ordering::SeqCst), refuse, "GR refusal was not recorded");
    }
    }
    }
    }
}
'''.replace('TAIL', tail)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'probe.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                            str(path / 'probe.rs'), '-o', str(path / 'probe')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'probe')], check=True)


if __name__ == '__main__':
    unittest.main()
