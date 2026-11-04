"""Exercise the production diagnostic window with counted protected snapshots."""
from pathlib import Path
import subprocess
import sys
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ProbeWindow(unittest.TestCase):
    def test_complete_window_gates_stage_records_on_real_after_snapshot(self):
        source = (ROOT / 'kernel/src/gpu/gk208/mod.rs').read_text()
        block = item(source, '        if native::memory_requested()')
        fixture = r'''
use std::cell::RefCell;
thread_local! { static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }
fn event(s: String) { EVENTS.with(|v| v.borrow_mut().push(s)); }
#[macro_export] macro_rules! serial_println { ($($args:tt)*) => { event(format!($($args)*)) }; }
pub struct Registers { pub hashes: usize, pub corrupt: bool, pub fail: bool }
pub mod pci { pub struct Device; }
pub struct ConsoleSurface;
pub mod memory {
    pub struct Object;
    impl Object { pub fn guarded(&self) -> u64 { 8 } }
    pub struct Stats { pub reserved: u64, pub charged: u64, pub free: u64 }
    pub struct Allocator;
    impl Allocator {
        pub fn stats(&self) -> Stats { Stats { reserved: 8, charged: 8, free: 16 } }
        pub fn reserved(&self) -> &[u64] { &[8] }
    }
    pub struct Layout { pub allocator: Allocator, pub objects: Vec<Object> }
}
pub mod timer { pub fn rdtsc() -> u64 { 0 } }
pub mod channel { pub fn elapsed(_: u64) -> u64 { 0 } }
pub mod native { pub fn memory_requested() -> bool { true } }
pub mod inventory {
    use super::*;
    pub fn digest(r: &mut Registers, total: u64, ranges: &[u64]) -> Result<(u64,String), &'static str> {
        assert_eq!(total, 32); assert_eq!(ranges, &[8,8]);
        r.hashes += 1; event(format!("hash{}", r.hashes));
        Ok((16, if r.corrupt && r.hashes == 2 { "b" } else { "a" }.repeat(64)))
    }
    pub fn probe(_: &mut Registers, _: &pci::Device, _: &mut memory::Layout, _: &ConsoleSurface) -> Result<String, &'static str> {
        event("inventory".into()); Ok("{\"stage\":\"inventory\"".into())
    }
}
pub mod vm_probe {
    use super::*;
    pub fn probe(r: &mut Registers, _: &pci::Device, _: &mut memory::Layout, _: &ConsoleSurface) -> Result<String, &'static str> {
        event("vm".into()); if r.fail { Err("VM failed") } else { Ok("{\"stage\":\"vm\"".into()) }
    }
}
pub mod aperture_probe {
    use super::*;
    pub fn probe(_: &mut Registers, _: &pci::Device, _: &mut memory::Layout, _: &ConsoleSurface) -> Result<String, &'static str> {
        event("aperture".into()); Ok("{\"stage\":\"aperture\"".into())
    }
}
fn run(corrupt: bool, fail: bool) -> Result<(), &'static str> {
    let mut regs = Registers { hashes: 0, corrupt, fail };
    let device = pci::Device;
    let mut layout = memory::Layout { allocator: memory::Allocator, objects: vec![memory::Object] };
    let console = ConsoleSurface;
    BLOCK
    Ok(())
}
#[test] fn shared_hashes_bracket_three_diagnostics_and_gate_records() {
    for (corrupt, fail) in [(false,false), (true,false), (false,true)] {
        EVENTS.with(|v| v.borrow_mut().clear());
        let result = run(corrupt, fail);
        let events = EVENTS.with(|v| v.borrow().clone());
        if fail { assert!(result.is_err()); }
        else {
            assert_eq!(events.iter().filter(|v| v.starts_with("hash")).count(), 2);
            let index = |s: &str| events.iter().position(|v| v == s).unwrap();
            assert!(index("hash1") < index("inventory"));
            assert!(index("inventory") < index("vm"));
            assert!(index("vm") < index("aperture"));
            assert!(index("aperture") < index("hash2"));
            if corrupt {
                assert!(result.is_err());
                assert!(events.iter().any(|v| v.contains("diagnostics=inventory,vm-lifetime,aperture") && v.contains("mismatch")), "missing attribution: {events:?}");
            } else {
                assert!(result.is_ok());
                for prefix in ["GT710_VRAM {", "GT710_VM {", "GT710_APERTURE {"] {
                    let record = events.iter().find(|v| v.starts_with(prefix)).expect("missing stage evidence");
                    assert!(events.iter().position(|v| v == record).unwrap() > index("hash2"));
                    assert!(record.contains("\"protected_bytes\":16"));
                    assert!(record.contains(&format!("\"protected_before\":\"{}\"", "a".repeat(64))));
                    assert!(record.contains(&format!("\"protected_after\":\"{}\"", "a".repeat(64))));
                    assert!(record.ends_with('}'));
                }
            }
        }
        if corrupt || fail { assert!(!events.iter().any(|v| ["GT710_VRAM {", "GT710_VM {", "GT710_APERTURE {"].iter().any(|p| v.starts_with(p)))); }
    }
}
'''.replace('BLOCK', block)
        evidence = Path(sys.argv[1])
        evidence.mkdir(parents=True, exist_ok=True)
        path = evidence / 'probe-window.rs'
        binary = evidence / 'probe-window'
        path.write_text(fixture)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test', str(path), '-o', str(binary)], check=True)
        subprocess.run([str(binary)], check=True)


if __name__ == '__main__':
    unittest.main(argv=[sys.argv[0]])
