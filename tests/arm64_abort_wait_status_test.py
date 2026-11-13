"""Exercise the ARM data-abort exit calculation through shared wait encoding."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class AbortWaitStatus(unittest.TestCase):
    def test_uncaught_data_abort_preserves_core_dump_status(self):
        arm = (ROOT / 'kernel/src/arch_impl/aarch64/exception.rs').read_text()
        delivery = (ROOT / 'kernel/src/signal/delivery.rs').read_text()
        types = (ROOT / 'kernel/src/signal/types.rs').read_text()
        wait = (ROOT / 'kernel/src/syscall/wait.rs').read_text()
        status, = re.findall(r'let el0_status = [^;]+;', arm)
        constants = ROOT / 'kernel/src/signal/constants.rs'
        harness = f'''
pub mod signal {{
    #[path = "{constants}"]
    pub mod constants;
    pub mod types {{
        use super::constants::*;
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        {item(types, 'pub enum SignalDefaultAction')}
        {item(types, 'pub fn default_action(')}
    }}
    pub mod delivery {{
        use super::types::{{default_action, SignalDefaultAction}};
        {item(delivery, 'pub fn fatal_exit_code(')}
        {item(delivery, 'pub fn fault_death_exit_code(')}
    }}
}}
{item(arm, 'fn el0_abort_signal(')}
{item(wait, 'enum Event {').replace('enum Event', 'pub enum Event')}
{item(wait, 'fn wait_status(')}
fn abort_exit_code(fsc: u32) -> i32 {{
    let (el0_sig, _) = el0_abort_signal(fsc);
    {status}
    el0_status
}}
fn main() {{
    // Unmapped access (SIGSEGV) and misaligned access (SIGBUS).
    for (fsc, expected) in [(0x04, 139), (0x21, 135)] {{
        let status = wait_status(Event::Exited(abort_exit_code(fsc)));
        assert_eq!(status, expected, "ARM uncaught data abort fsc={{fsc:#x}}");
        let (sig, _) = el0_abort_signal(fsc);
        assert_eq!(status, wait_status(Event::Exited(
            signal::delivery::fault_death_exit_code(sig))), "x86 fault-path parity");
    }}
}}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            source = path / 'abort_wait.rs'
            binary = path / 'abort_wait'
            source.write_text(harness)
            policy = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(policy + ['rustc', '--edition=2021', '-Dwarnings',
                                     str(source), '-o', str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
