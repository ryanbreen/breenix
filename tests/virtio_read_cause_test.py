#!/usr/bin/env python3
"""E24 ledger 520(r): execute ARM wrapper error translation on the host."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
REASON = 'Block MMIO request already in progress'


def block(source, start):
    at = source.index('{', start)
    depth = 1
    end = at + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class VirtioReadCause(unittest.TestCase):
    def test_arm_wrapper_preserves_driver_failure(self):
        wrapper = (ROOT / 'kernel/src/block/virtio.rs').read_text()
        arm = wrapper.index('// ARM64 version:')
        method = block(wrapper, wrapper.index('fn read_blocks(', arm))
        errors = (ROOT / 'kernel/src/block/mod.rs').read_text()
        enum = '#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n' + block(errors, errors.index('pub enum BlockError'))
        detail = ROOT / 'kernel/src/block/read_failure.rs'
        if detail.exists():
            types = f'#[path = "{detail}"] pub mod read_failure;\nuse read_failure::VirtioReadFailure;'
            failure = '''VirtioReadFailure {
                reason: "Block MMIO request already in progress", stage: "gate-cannot-sleep",
                device: 0, sector: 444976, bytes: 16384,
                thread: Some(73), preempt_count: 0, kill_custody: Some(1), in_interrupt: false,
                interrupts_enabled: true, timer_ready: true,
                gate_locked: true, gate_wedged: false, pending_token: 17, completed_token: 0, waiter_thread: 0,
                completed_desc: u32::MAX, completed_status: u32::MAX,
                used_idx: 9, last_used_idx: 9, device_status: 15,
            }'''
            error_type = 'VirtioReadFailure'
        else:
            types, failure, error_type = '', f'"{REASON}"', "&'static str"
        program = f'''
{types}
{enum}
mod block_driver {{
    {"use super::VirtioReadFailure;" if detail.exists() else ""}
    pub fn read_sectors(_: usize, _: u64, _: &mut [u8]) -> Result<(), {error_type}> {{ Err({failure}) }}
}}
struct VirtioBlockWrapper {{ block_size: usize, device_index: usize }}
impl VirtioBlockWrapper {{
    fn num_blocks(&self) -> u64 {{ 1_000_000 }}
    {method}
}}
fn main() {{
    let wrapper = VirtioBlockWrapper {{ block_size: 512, device_index: 0 }};
    let error = wrapper.read_blocks(444976, 32, &mut [0; 16384]).unwrap_err();
    let actual = format!("{{error:?}}");
    for expected in ["{REASON}", "gate-cannot-sleep", "preempt_count: 0",
        "pending_token: 17", "completed_desc: 4294967295", "completed_status: 4294967295",
        "device_status: 15"] {{ assert!(actual.contains(expected), "lost {{expected}}: {{actual}}"); }}
}}
'''
        with tempfile.TemporaryDirectory() as tmp:
            source, binary = Path(tmp) / 'probe.rs', Path(tmp) / 'probe'
            source.write_text(program)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(source), '-o', str(binary)], check=True, timeout=45)
            subprocess.run([str(binary)], check=True, timeout=5)


if __name__ == '__main__':
    unittest.main()
