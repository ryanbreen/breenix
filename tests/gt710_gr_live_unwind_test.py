"""Inject errors into production live-channel orchestration and FIFO teardown.

Only pre-live allocation and command DMA are substituted. The production
constructor tail, activation, execute, drain, restoration and reset ordering run
against registers with nonzero saved values; no GPU execution is claimed.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def item(source, marker):
    start = source.index(marker)
    opening = source.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class LiveUnwind(unittest.TestCase):
    def test_live_failures_restore_before_reset_and_ce_refuses_bindings(self):
        gr = (ROOT / 'kernel/src/gpu/gk208/gr/mod.rs').read_text()
        channel = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        session = item(channel, 'pub(in crate::gk208::gr) struct Session')
        nonce = item(channel, 'pub fn receipt_nonce(')
        methods = item(channel, '    impl Session {')
        new = item(methods, 'pub(in crate::gk208::gr) fn new(')
        prelude = '''
            let _ = (golden, code_address);
            step("graphics-new")?;
            let context = Context;
            let commands = Allocation::at(0x100000);
            let image = Allocation::at(0x200000);
            let pages = ManuallyDrop::new(system_dma::Pages);
            let (pbdma, bar_inst, bar_dir, pte, displacement) = (0, 0, 0, 0x900000, 0);
            let top = [0; 64]; let (engine, runlist, enabled, instance) = (0, 1, h.regs.read(0x204), 0x300000);
'''
        start = new.index('{') + 1
        methods = methods.replace(new, new[:start] + prelude + new[new.index('            let epoch ='):])
        issue = item(methods, '        fn issue_words(')
        methods = methods.replace(issue, issue[:issue.index('{') + 1] + '''
            let _ = (h, poison, budget, stop_on_trap, self.bar, USERD);
            let epoch = if sequence < 5 {u64::from(command[0]) | (u64::from(command[1])<<32)} else {self.report.identity.epoch};
            assert!(self.bound && !self.quiesced, "submission on a stopped graphics channel");
            step(&format!("issue-{sequence}"))?;
            let reject = INJECT.with(|i| i.borrow().target == format!("reject-{sequence}"));
            Ok(Observation { value: if sequence == 1 || reject { poison } else { nonce }, epoch,
                consumed: true, elapsed_us: 10, accepted: !reject && sequence != 1 && epoch == self.report.identity.epoch })
        }''')
        # Retain the actual issue wrapper/execute/bind/cleanup ordering.
        methods = methods.replace('super::super::binding', 'binding')
        # Include new cleanup methods automatically; substitute only issue DMA.
        methods = methods.replace('pub(in crate::gk208::gr)', 'pub(crate)')
        session = session.replace('pub(in crate::gk208::gr)', 'pub(crate)')
        start = gr.index('    let generated = context::generate(')
        if '    let outcome = (||' in gr:
            start = gr.index('    let outcome = (||')
        stop = gr.index('    if layout.allocator.stats() != baseline', start)
        flow = gr[start:stop].replace("super::ce::readback::Reader", "Reader")
        finish = item(gr, '    fn finish(mut self)').replace('super::channel::VmIo', 'VmIo')
        probe = item(gr, 'pub(super) fn probe(')
        probe_finish = probe[probe.index('    // This also runs'):probe.rindex('}')]
        # Pre-live helpers are separate from the live operations under test.
        harness = (ROOT / 'tests/fixtures/gt710_gr_live_unwind.rs').read_text()
        for key, value in [('SESSION', nonce + '\n' + session + '\n' + methods), ('FLOW', flow),
                           ('FINISH', finish), ('PROBE_FINISH', probe_finish),
                           ('CLEANUP', gr[gr.index('struct ProbeCleanup'):gr.index('/// Capture startup')])]:
            harness = harness.replace('// ' + key, value)
        readback = (ROOT / 'kernel/src/gpu/gk208/ce/readback.rs').read_text()
        reader_type = item(readback, 'pub(in crate::gk208) struct Reader')
        quiesce = item(readback, '    pub(in crate::gk208) fn quiesce(')
        harness = harness.replace('// READER_QUIESCE',
            reader_type.replace('pub(in crate::gk208)', '') + '\nimpl Reader {\n' +
            quiesce.replace('pub(in crate::gk208)', '') + '\n}')
        # Exercise CE preflight from the real entry points, up to allocation.
        ce = (ROOT / 'kernel/src/gpu/gk208/ce.rs').read_text()
        helper = item(ce, 'fn ensure_unbound(') if 'fn ensure_unbound(' in ce else ''
        begin = item((ROOT / 'kernel/src/gpu/gk208/ce/process.rs').read_text(), '    fn begin(')
        begin = begin.replace('let mut regs=Registers(self.registers)', 'let mut regs=unsafe { &mut *(self.registers as *mut Registers) }')
        begin = begin[:begin.index('        let baseline=')] + '\n let _ = old; Ok(())\n}'
        reader = item((ROOT / 'kernel/src/gpu/gk208/ce/readback.rs').read_text(),
                      '    pub(in crate::gk208) fn new(')
        body = reader[reader.index('{') + 1:reader.index('        let status =')]
        # The constructor itself must independently reject bound storage.
        constructor = item(ce, '    fn new(regs:')
        guard = constructor[constructor.index('{') + 1:constructor.index('        let mut io=')]
        harness = harness.replace('// CE_HELPER', helper)
        harness = harness.replace('// CE_BEGIN', begin)
        harness = harness.replace('// CE_READER', body + '\n let _ = (total, engine); Ok(())')
        harness = harness.replace('// CE_CONSTRUCTOR', guard + '\n let _ = (regs, id); Ok(())')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'live.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                            str(path / 'live.rs'), '-o', str(path / 'live')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'live'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
