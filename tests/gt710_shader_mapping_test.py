"""Run the production GR mapping loop and inspect the resulting PTE flags.

Nouveau v6.8 MIT vmmgf100.c:249-265,314-316 distinguishes ordinary
program data (priv=0) from GR's internal context buffers (gf100.c:383).
The test deliberately varies the program VA and backing order.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ShaderMapping(unittest.TestCase):
    def test_program_is_readable_without_privilege_and_context_stays_private(self):
        channel = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        start = channel.index('            let flags = Flags {')
        end = channel.index('            let commands = h.layout.allocator.allocate', start)
        mapping = channel[start:end]
        vm = (ROOT / 'kernel/src/gpu/gk208/vm.rs').read_text()
        start = vm.index('#[derive(Clone, Copy, Debug, Default)]')
        end = vm.index('/// Physical VRAM access', start)
        flags = vm[start:end]
        harness = r'''
FLAGS
#[derive(Clone, Copy)] struct Allocation(u64);
struct Layout { allocator: () }
struct Hardware<'a> { regs: &'a mut (), total: u64, layout: Layout, backing: Vec<(u64, Allocation)> }
struct VmIo<'a> { regs: &'a mut (), total: u64 }
struct Context { mappings: Vec<(u64,u64)> }
impl Context {
    fn map(&mut self, _: &mut (), io: &mut VmIo<'_>, va: u64, a: Allocation, flags: Flags) -> Result<(), &'static str> {
        let _ = (&io.regs, io.total);
        self.mappings.push((va, (a.0 >> 8) | flags.bits())); Ok(())
    }
}
fn mapped(code_address: u64) -> Result<Vec<(u64,u64)>, &'static str> {
    let _ = code_address;
    let mut regs = ();
    let mut h = Hardware { regs: &mut regs, total: 1<<30, layout: Layout { allocator: () },
        backing: vec![(0x10002000,Allocation(0x800000)), (0x10000000,Allocation(0x900000))] };
    let mut context = Context { mappings: vec![] };
MAPPING
    Ok(context.mappings)
}
#[test] fn program_fetch_permissions() {
    for code in [0x10000000,0x10002000] {
        let mappings = mapped(code).unwrap();
        for (va,pte) in mappings {
            assert_eq!(pte & 2, if va==code {0} else {2}, "PTE privilege at {va:#x}, program {code:#x}");
            assert_eq!(pte & ((1<<32)|1), (1<<32)|1);
        }
    }
}
'''.replace('FLAGS', flags).replace('MAPPING', mapping)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'mapping.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path / 'mapping.rs'), '-o', str(path / 'mapping')], check=True)
            subprocess.run([str(path / 'mapping'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
