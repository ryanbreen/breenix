"""Full PRAMIN snapshots must spend PCIe reads on data, not repeated selection."""
from pathlib import Path
import subprocess
import sys
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ProbeStartup(unittest.TestCase):
    def test_snapshot_reads_each_byte_with_bank_scaled_selector_cost(self):
        registers = (ROOT / 'kernel/src/gpu/gk208/mod.rs').read_text()
        adapter = item(registers, 'impl memory::Access for Registers')
        pair = item(adapter, '    fn read_pair') if '    fn read_pair' in adapter else ''
        fixture = r'''
extern crate alloc;
#[path="MEMORY"] pub mod memory;
use memory::{Access, Pramin, Range};
use std::cell::Cell;
struct Registers(usize, Cell<usize>);
impl Registers {
    fn read(&self, reg: usize) -> u32 {
        self.1.set(self.1.get() + 1);
        unsafe { ((self.0 + reg) as *const u32).read_volatile() }
    }
}
impl Access for Registers {
    PAIR
    fn read(&mut self, reg: usize) -> u32 { Registers::read(self, reg) }
    fn write(&mut self, _: usize, _: u32) { panic!("pair read wrote storage"); }
}
struct Aperture { bank: u32, selector_reads: usize, words: usize, held: bool, stuck: bool }
impl Access for Aperture {
    fn with_pramin<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        assert!(!self.held); self.held = true;
        let saved = self.bank;
        let result = f(self);
        self.bank = saved; self.held = false;
        result
    }
    fn read(&mut self, reg: usize) -> u32 {
        if reg == 0x1700 { self.selector_reads += 1; return self.bank; }
        assert!(self.held, "snapshot escaped selector serialization");
        assert!((0x700000..0x800000).contains(&reg));
        self.words += 1;
        let address = (u64::from(self.bank) << 16) + (reg - 0x700000) as u64;
        (address as u32).wrapping_mul(0x10203) ^ 0xaabbccdd
    }
    fn write(&mut self, reg: usize, value: u32) {
        assert!(self.held); assert_eq!(reg, 0x1700);
        if !self.stuck { self.bank = value; }
    }
}
#[test]
fn snapshot_keeps_complete_bytes_edges_banks_and_restoration_at_bounded_io_cost() {
    for (start, size) in [(0, 1 << 20), (0xffffc, 1 << 20), (0x1ffffc, 4)] {
        let mut a = Aperture { bank: 0x1234, selector_reads: 0, words: 0, held: false, stuck: false };
        let mut p = Pramin::new(&mut a, 2 << 20);
        let bytes = p.bytes(Range { start, size }).unwrap();
        assert_eq!(Access::read(p.access(), 0x1700), 0x1234);
        drop(p);
        let expected: Vec<u8> = (start..start + size).step_by(4)
            .flat_map(|v| ((v as u32).wrapping_mul(0x10203) ^ 0xaabbccdd).to_le_bytes()).collect();
        assert_eq!(bytes, expected);
        assert_eq!(a.words as u64 * 4, size, "no byte may be sampled or omitted");
        assert_eq!(a.bank, 0x1234);
        assert!(a.selector_reads <= 8, "snapshot rereads its held selector per word: {}", a.selector_reads);
        let before = a.words;
        let mut p = Pramin::new(&mut a, 2 << 20);
        assert!(p.bytes(Range { start: (2 << 20) - 4, size: 8 }).is_err());
        drop(p); assert_eq!(a.words, before);
    }
    let mut a = Aperture { bank: 0x1234, selector_reads: 0, words: 0, held: false, stuck: true };
    assert!(Pramin::new(&mut a, 2 << 20).bytes(Range { start: 0, size: 4096 }).is_err());
    assert_eq!(a.words, 0, "a refused bank must not yield snapshot evidence");
}
#[test]
fn hardware_pair_uses_one_aligned_wide_storage_load() {
    let words = [0xaabbccdd12345678u64];
    let mut registers = Registers(words.as_ptr() as usize, Cell::new(0));
    assert_eq!(Access::read_pair(&mut registers, 0), 0xaabbccdd12345678);
    assert_eq!(registers.1.get(), 0, "hardware pair fell back to two PCIe scalar reads");
}
'''.replace('MEMORY', str(ROOT / 'kernel/src/gpu/gk208/memory.rs')).replace('PAIR', pair)
        # The caller supplies its retained evidence directory; no capture goes to /tmp.
        evidence = Path(sys.argv[1])
        evidence.mkdir(parents=True, exist_ok=True)
        source = evidence / 'probe-startup.rs'
        binary = evidence / 'probe-startup'
        source.write_text(fixture)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test', str(source), '-o', str(binary)], check=True)
        subprocess.run([str(binary)], check=True)


if __name__ == '__main__':
    unittest.main(argv=[sys.argv[0]])
