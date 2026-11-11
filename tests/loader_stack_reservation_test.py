"""Exercise the loader's UEFI stack reservation with an occupied-page allocator."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LoaderStackReservationTest(unittest.TestCase):
    def test_reserve_before_loading_and_refuse_owned_pages(self):
        source = (ROOT / 'parallels-loader/src/main.rs').read_text()
        found = re.search(r'fn reserve_loader_stack\(.*?\n\}', source, re.S)
        self.assertIsNotNone(found, 'loader must reserve its stack from UEFI')
        handoff = source[source.index('fn main()'):source.index('fn reserve_loader_stack(')]
        call = handoff.index('if let Err(e) = reserve_loader_stack(ram_base_offset)')
        self.assertLess(call, handoff.index('kernel_load::load_kernel('))
        self.assertLess(call, handoff.index('uefi::boot::exit_boot_services('))
        code = re.sub(r'"(?:\\.|[^"\\])*"', '""', handoff[call:])
        self.assertRegex(code, r'reserve_loader_stack\(ram_base_offset\)\s*\{[^}]*halt\(\);')
        contract = (ROOT / 'libs/arm64-boot-contract/src/lib.rs').read_text()
        constants = '\n'.join(re.findall(r'pub const LOADER_BOOT_STACK_(?:BOTTOM|TOP)_PHYS: u64 = [^;]+;', contract))
        harness = r'''
extern crate self as uefi;
extern crate self as arm64_boot_contract;
__CONSTANTS__
pub type Result<T> = std::result::Result<T, &'static str>;
pub mod mem { pub mod memory_map {
    #[derive(Debug, PartialEq)] pub struct MemoryType;
    impl MemoryType { pub const LOADER_DATA: Self = Self; }
} }
pub mod boot {
    use super::mem::memory_map::MemoryType;
    use std::cell::Cell;
    #[derive(Debug, PartialEq)] pub enum AllocateType { Address(u64) }
    thread_local! {
        pub static OCCUPIED: Cell<bool> = const { Cell::new(false) };
        pub static EXPECTED: Cell<u64> = const { Cell::new(0) };
    }
    pub fn allocate_pages(ty: AllocateType, kind: MemoryType, pages: usize) -> super::Result<()> {
        assert_eq!(kind, MemoryType::LOADER_DATA);
        assert_eq!(pages, 512);
        let AllocateType::Address(address) = ty;
        assert_eq!(address, EXPECTED.with(Cell::get));
        if OCCUPIED.with(|v| v.replace(true)) { Err("owned by firmware") } else { Ok(()) }
    }
}
use mem::memory_map::MemoryType;
__FUNCTION__
#[test]
fn reserves_linked_and_relocated_stack_and_propagates_collision() {
    for offset in [0, 0x40000000] {
        boot::OCCUPIED.with(|v| v.set(false));
        boot::EXPECTED.with(|v| v.set(0x42e00000 + offset));
        assert_eq!(reserve_loader_stack(offset), Ok(()));
        assert_eq!(reserve_loader_stack(offset), Err("owned by firmware"));
    }
}
'''.replace('__CONSTANTS__', constants).replace('__FUNCTION__', found.group())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'reservation.rs').write_text(harness)
            subprocess.run(['rustc', '--edition=2021', '--test', '-Dwarnings',
                            str(path / 'reservation.rs'), '-o', str(path / 'reservation')], check=True)
            subprocess.run([str(path / 'reservation')], check=True)


if __name__ == '__main__':
    unittest.main()
