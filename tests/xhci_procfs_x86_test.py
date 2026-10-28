#!/usr/bin/env python3
"""Compile the procfs xHCI routes and formatter for x86, then execute them.

The harness retains the source cfg attributes and match arms. Hardware reads
use distinct fixture values; the formatter itself comes from the kernel file.
On Apple Silicon the x86 host executable runs through Rosetta, without a VM.
"""
from pathlib import Path
import platform
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class XhciProcfsX86(unittest.TestCase):
    def test_routes_and_counter_output(self):
        source = (ROOT / "kernel/src/fs/procfs/mod.rs").read_text()
        attrs = r'(?:^[ \t]*#\[[^\n]+\]\n)*'
        module = re.search(attrs + r'^mod xhci;', source, re.M).group()
        arms = re.findall(
            attrs + r'^[ \t]*ProcEntryType::Xhci(?:Trace|Counters) => text\([^\n]+',
            source, re.M,
        )
        self.assertTrue(arms, "procfs xHCI match arms are missing")
        generator = ROOT / "kernel/src/fs/procfs/xhci.rs"
        module = module.replace('mod xhci;', f'#[path = "{generator}"] mod xhci;')
        harness = r'''
extern crate alloc;
use alloc::string::String;
pub mod drivers {
    pub mod hid_boot {
        pub static NONZERO_KBD_COUNT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(14);
    }
    pub mod xhci {
        pub fn state() -> &'static str { "running" }
        pub fn is_initialized() -> bool { true }
        pub fn interrupt_count() -> u64 { 15 }
        pub mod host_keys {
            use core::sync::atomic::AtomicU64;
            pub static QUEUED: AtomicU64 = AtomicU64::new(21);
            pub static REPLAYED: AtomicU64 = AtomicU64::new(22);
            pub static DISPATCHED: AtomicU64 = AtomicU64::new(23);
            pub static CHANGED: AtomicU64 = AtomicU64::new(24);
            pub static REPLAY_MAX_NS: AtomicU64 = AtomicU64::new(3_000_000_025);
        }
    }
}
pub mod tracing { pub mod providers { pub mod xhci {
    pub struct Counter(pub u64);
    impl Counter { pub fn aggregate(&self) -> u64 { self.0 } }
    pub static XHCI_MSI_EVENT_TOTAL: Counter = Counter(11);
    pub static XHCI_IRQ_ENTRY_TOTAL: Counter = Counter(12);
    pub static XHCI_LOCK_CONTENDED_TOTAL: Counter = Counter(13);
} } }
__MODULE__
#[path = "__GENERATOR__"] mod direct_generator;
enum ProcEntryType { XhciTrace, XhciCounters }
fn text(value: String) -> String { value }
fn read(entry: ProcEntryType) -> String {
    match entry { __ARMS__ }
}
fn check_counters(output: &str) {
    for field in ["XHCI_MSI_EVENT_TOTAL=11", "XHCI_IRQ_ENTRY_TOTAL=12",
        "XHCI_LOCK_CONTENDED_TOTAL=13", "KBD_NONZERO_TOTAL=14",
        "HOSTKEY_QUEUED=21", "HOSTKEY_REPLAYED=22", "HOSTKEY_DISPATCHED=23",
        "HOSTKEY_CHANGED=24", "HOSTKEY_REPLAY_MAX_NS=3000000025"] {
        assert!(output.split_whitespace().any(|value| value == field),
            "missing {field} in {output:?}");
    }
}
#[test]
fn x86_routes_to_live_generators() {
    assert_eq!(read(ProcEntryType::XhciTrace),
        "xhci state=running initialized=true interrupts=15\n");
    check_counters(&read(ProcEntryType::XhciCounters));
}
#[test]
fn generator_formats_replay_measurements() {
    check_counters(&direct_generator::generate_xhci_counters());
    assert_eq!(direct_generator::generate_xhci_trace(),
        "xhci state=running initialized=true interrupts=15\n");
}
'''.replace('__MODULE__', module).replace('__GENERATOR__', str(generator)).replace('__ARMS__', '\n'.join(arms))
        target = 'x86_64-apple-darwin' if platform.system() == 'Darwin' else 'x86_64-unknown-linux-gnu'
        with tempfile.TemporaryDirectory(prefix='xhci-procfs-x86-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(harness)
            command = ['rustc', '--edition=2021', '--test', '--target', target,
                       '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
            if platform.system() == 'Darwin':
                command = ['taskpolicy', '-b', *command]
            subprocess.run(command, check=True)
            subprocess.run([str(path / 'check'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
