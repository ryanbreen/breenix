"""Execute recovery's channel-construction calls around initial publication.

The real CE constructor preflight runs against live binding words. Allocation
and publication are modeled; this checks ownership ordering, not GPU execution.
"""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class RecoveryPreparation(unittest.TestCase):
    def test_replacements_are_prepared_before_old_channels_are_published(self):
        source = (ROOT / 'kernel/src/gpu/gk208/ce/recovery.rs').read_text()
        source = source[source.index('pub(super) fn probe('):]
        ce = (ROOT / 'kernel/src/gpu/gk208/ce.rs').read_text()
        helper = ce[ce.index('fn ensure_unbound('):ce.index('\nstruct Channel')]
        constructor = ce[ce.index('    fn new(regs:'):]
        guard = constructor[constructor.index('{') + 1:constructor.index('        let mut io=')]
        calls = []
        for match in re.finditer(r'let mut (?:fresh_)?[ab] = Channel::new\([^;]+;', source):
            call = match[0][match[0].index('Channel::new'):].replace(
                '&mut s.layout, s.total, ', '').replace('&status, ', '')
            calls.append((match.start(), call))
        self.assertEqual(len(calls), 5)
        # Only the four initial/replacement allocations matter; A's later
        # recreation follows confirmed removal and is exercised by recovery.
        calls = calls[:2] + [entry for entry in calls if 'fresh_userd' in entry[1]]
        publication = source.index('    activate_pair(')
        calls.append((publication, 'regs.bound = [0x80000100, 0x80000200];'))
        flow = '\n'.join(call for _, call in sorted(calls))
        harness = r'''
struct Registers { bound: [u32; 2] }
impl Registers { fn read(&mut self, address: usize) -> u32 { self.bound[(address-0x800000)/8] } }
HELPER
struct Channel;
impl Channel {
    fn new(regs: &mut Registers, id: u32, userd: u64) -> Result<Self, &'static str> {
GUARD
        assert!(userd == 0x1000 || userd == 0x2000);
        Ok(Self)
    }
}
fn run() -> Result<(), &'static str> {
    let mut regs = Registers { bound: [0; 2] };
    let (userd, fresh_userd) = (0x1000, 0x2000);
FLOW
    Ok(())
}
fn main() { run().expect("replacement preparation touched a live channel identity"); }
'''.replace('HELPER', helper).replace('GUARD', guard).replace('FLOW', flow)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'prepare.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'prepare.rs'),
                                      '-o', str(path / 'prepare')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'prepare')], check=True)


if __name__ == '__main__':
    unittest.main()
