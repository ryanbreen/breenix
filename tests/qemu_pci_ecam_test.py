"""Run the production QEMU PCI setup against host device-tree fixtures."""
from pathlib import Path
import json
import struct
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def tree(base, mmio=0x10000000, captured=None):
    strings = bytearray()
    body = bytearray()

    def words(*values):
        return struct.pack('>' + 'I' * len(values), *values)

    def padded(data):
        return data + b'\0' * (-len(data) % 4)

    def prop(name, value):
        offset = len(strings)
        strings.extend(name.encode() + b'\0')
        body.extend(words(3, len(value), offset) + padded(value))

    body.extend(words(1) + padded(b'\0'))
    prop('#address-cells', words(2))
    prop('#size-cells', words(2))
    if base is not None:
        body.extend(words(1) + padded(b'pcie@10000000\0'))
        prop('reg', words(*captured['reg']) if captured else
             words(base >> 32, base & 0xffffffff, 0,
                   0x1000000 if base < 2**32 else 0x10000000))
        prop('compatible', b'pci-host-ecam-generic\0')
        prop('#address-cells', words(3))
        prop('#size-cells', words(2))
        prop('ranges', words(*captured['ranges']) if captured else
             words(0x02000000, 0, mmio, 0, mmio, 0, 0x1000000))
        body.extend(words(2))
    body.extend(words(2, 9))
    total = 56 + len(body) + len(strings)
    return words(0xd00dfeed, total, 56, 56 + len(body), 40, 17, 16, 0,
                 len(strings), len(body)) + bytes(16) + body + strings


class QemuPciEcamTest(unittest.TestCase):
    def test_config_reads_and_writes_use_the_mapped_address(self):
        source = (ROOT / 'kernel/src/drivers/pci.rs').read_text()
        functions = []
        for name in ('pci_read_config_dword', 'pci_write_config_dword'):
            functions.append(source.split('#[cfg(target_arch = "aarch64")]\npub(crate) fn ' + name, 1)[1].split('\n}', 1)[0])
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            harness = directory / 'access.rs'
            harness.write_text('''use std::sync::atomic::{AtomicU64, Ordering};
mod platform_config {
    use super::*;
    pub static PHYS: AtomicU64 = AtomicU64::new(0);
    pub static VIRT: AtomicU64 = AtomicU64::new(0);
    pub fn pci_ecam_base() -> u64 { PHYS.load(Ordering::Relaxed) }
    pub fn pci_ecam_virt_base() -> u64 { VIRT.load(Ordering::Relaxed) }
}
pub(crate) fn pci_read_config_dword''' + functions[0] + '\n}\n' +
                               'pub(crate) fn pci_write_config_dword' + functions[1] + '''
}
fn main() {
    let mut mapped = vec![0u32; 1024];
    let mut decoy = vec![0u32; 1024];
    mapped[4] = 0x12345678;
    platform_config::PHYS.store((decoy.as_mut_ptr() as u64).wrapping_sub(0xffff000000000000), Ordering::Relaxed);
    platform_config::VIRT.store(mapped.as_mut_ptr() as u64, Ordering::Relaxed);
    assert_eq!(pci_read_config_dword(0, 0, 0, 0x10), 0x12345678);
    pci_write_config_dword(0, 0, 0, 0x14, 0x87654321);
    assert_eq!(mapped[5], 0x87654321);
    assert_eq!(decoy[5], 0);
}
''')
            binary = directory / 'access'
            subprocess.run(['rustc', '--edition=2021', '-C', 'overflow-checks=no', str(harness), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

    def test_setup_uses_advertised_windows_and_falls_back_only_without_a_node(self):
        source = (ROOT / 'kernel/src/platform_config.rs').read_text()
        setup = source.split('pub fn init_qemu_pci() {', 1)[1].split('\n}', 1)[0]
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            harness = directory / 'setup.rs'
            harness.write_text('''
use std::sync::{atomic::{AtomicU64, Ordering}, OnceLock};
#[path = "''' + str(ROOT / 'libs/arm64-boot-contract/src/phys_memory.rs') + '''"]
pub mod phys_memory;
pub use phys_memory::{fdt, FdtError};
static DTB: OnceLock<Vec<u8>> = OnceLock::new();
pub fn qemu_dtb() -> Result<&'static [u8], FdtError> { Ok(DTB.get().unwrap()) }
fn map_qemu_ecam(base: u64, size: u64) -> Option<u64> {
    assert!(base < 1 << 40 && size <= 0x10000000);
    Some(0xffffdffff0000000)
}
static PCI_ECAM_VIRT: AtomicU64 = AtomicU64::new(0);
static PCI_ECAM_BASE: AtomicU64 = AtomicU64::new(0);
static PCI_ECAM_SIZE: AtomicU64 = AtomicU64::new(0);
static PCI_MMIO_BASE: AtomicU64 = AtomicU64::new(0);
static PCI_MMIO_SIZE: AtomicU64 = AtomicU64::new(0);
static PCI_BUS_START: AtomicU64 = AtomicU64::new(0);
static PCI_BUS_END: AtomicU64 = AtomicU64::new(0);
fn init_qemu_pci() {''' + setup + '''
}
fn main() {
    DTB.set(std::fs::read(std::env::args().nth(1).unwrap()).unwrap()).unwrap();
    init_qemu_pci();
    println!("{:x} {:x} {:x} {:x} {:x}", PCI_ECAM_BASE.load(Ordering::Relaxed),
             PCI_ECAM_SIZE.load(Ordering::Relaxed), PCI_MMIO_BASE.load(Ordering::Relaxed),
             PCI_MMIO_SIZE.load(Ordering::Relaxed), PCI_ECAM_VIRT.load(Ordering::Relaxed));
}
''')
            binary = directory / 'setup'
            subprocess.run(['rustc', '--edition=2021', str(harness), '-o', str(binary)], check=True)
            for base, expected in ((0x4010000000, '4010000000 10000000 20000000 1000000'),
                                   (0x3f000000, '3f000000 1000000 20000000 1000000'),
                                   (None, '4010000000 10000000 10000000 2eff0000')):
                with self.subTest(base=base):
                    fixture = directory / 'tree.dtb'
                    fixture.write_bytes(tree(base, mmio=0x20000000))
                    result = subprocess.check_output([str(binary), str(fixture)], text=True)
                    self.assertEqual(result.strip(), expected + ' ffffdffff0000000')
            # Captured with QEMU 10.0.12: -M virt,gic-version=3[,highmem=off],dumpdtb=...
            # Headless TCG, max CPU, 512 MiB. PCI properties: tests/fixtures/qemu-pci/layouts.json.
            layouts = json.loads((ROOT / 'tests/fixtures/qemu-pci/layouts.json').read_text())
            for name, base, size in (('default', 0x4010000000, 0x10000000),
                                      ('low', 0x3f000000, 0x1000000)):
                fixture.write_bytes(tree(base, captured=layouts[name]))
                result = subprocess.check_output([str(binary), str(fixture)], text=True)
                self.assertEqual(result.strip(), f'{base:x} {size:x} 10000000 2eff0000 ffffdffff0000000')
            for base in (0x90000000, 0x200000000, 0x4080000000):
                fixture.write_bytes(tree(base))
                result = subprocess.check_output([str(binary), str(fixture)], text=True)
                self.assertEqual(result.strip(), f'{base:x} {0x1000000 if base < 2**32 else 0x10000000:x} 10000000 1000000 ffffdffff0000000')
            for base, mmio in ((0x4010000000, 0x80000000), (1 << 40, 0x10000000),
                               (0x3f000001, 0x10000000)):
                fixture.write_bytes(tree(base, mmio=mmio))
                self.assertEqual(subprocess.check_output([str(binary), str(fixture)], text=True).strip(), '0 0 0 0 0')
            fixture.write_bytes(b'bad device tree')
            self.assertEqual(subprocess.check_output([str(binary), str(fixture)], text=True).strip(), '0 0 0 0 0')


if __name__ == '__main__':
    unittest.main()
