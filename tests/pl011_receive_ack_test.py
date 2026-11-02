#!/usr/bin/env python3
"""Exercise the production acknowledgement with an arrival after the empty poll."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class Pl011ReceiveAck(unittest.TestCase):
    def test_late_byte_keeps_its_receive_interrupt(self):
        source = (ROOT / 'kernel/src/serial_aarch64.rs').read_text()
        start = source.index('pub fn clear_rx_interrupt()')
        end = source.index('\n}', start) + 2
        acknowledge = source[start:end]
        harness = r'''
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
mod reg { pub const ICR: usize = 0x44; }
mod int { pub const RX: u32 = 1 << 4; pub const RT: u32 = 1 << 6; }
static UART_16550: AtomicBool = AtomicBool::new(false);
static RIS: AtomicU32 = AtomicU32::new(0);
static WRITES: AtomicU32 = AtomicU32::new(0);
fn is_16550() -> bool { UART_16550.load(Ordering::Relaxed) }
fn write_reg(offset: usize, value: u32) {
    assert_eq!(offset, reg::ICR);
    WRITES.fetch_add(1, Ordering::Relaxed);
    RIS.fetch_and(!value, Ordering::Relaxed);
}
ACKNOWLEDGE
fn main() {
    // The receiver reported empty, then a byte filled the one-byte holding
    // register before acknowledgement. RXRIS belongs to that unread byte.
    RIS.store(int::RX | int::RT, Ordering::Relaxed);
    clear_rx_interrupt();
    assert_ne!(RIS.load(Ordering::Relaxed) & int::RX, 0,
               "acknowledgement erased the unread byte's RX event");
    assert_eq!(RIS.load(Ordering::Relaxed) & int::RT, 0);
    let writes = WRITES.load(Ordering::Relaxed);
    UART_16550.store(true, Ordering::Relaxed);
    clear_rx_interrupt();
    assert_eq!(WRITES.load(Ordering::Relaxed), writes);
}
'''.replace('ACKNOWLEDGE', acknowledge)
        with tempfile.TemporaryDirectory(prefix='pl011-ack-') as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
