"""Execute the production UART handler with a receive arriving during TX service."""
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class SerialIrq(unittest.TestCase):
    def test_receive_behind_transmit_interrupt_is_drained_before_eoi(self):
        source = (ROOT / 'kernel/src/interrupts.rs').read_text()
        start = source.index('{', source.index('fn serial_interrupt_handler('))
        depth = 1
        end = start + 1
        while depth:
            depth += (source[end] == '{') - (source[end] == '}')
            end += 1
        body = source[start + 1:end - 1]
        harness = r'''
use std::sync::Mutex;
struct Uart { rx: Option<u8>, thre: bool, injected: bool, delivered: Vec<u8> }
static UART: Mutex<Uart> = Mutex::new(Uart {
    rx: None, thre: true, injected: false, delivered: Vec::new(),
});
struct Segment(u64);
struct InterruptStackFrame { code_segment: Segment }
enum InterruptIndex { Serial }
impl InterruptIndex { fn as_u8(self) -> u8 { 36 } }
mod task { pub mod thread {
    pub struct TrapTime;
    impl TrapTime { pub fn enter(_: bool) -> Self { Self } }
} }
mod per_cpu { pub fn irq_enter() {} pub fn irq_exit() {} }
mod x86_64 { pub mod instructions { pub mod port {
    pub struct Port<T>(u16, std::marker::PhantomData<T>);
    impl Port<u8> {
        pub fn new(address: u16) -> Self { Self(address, std::marker::PhantomData) }
        pub unsafe fn read(&mut self) -> u8 {
            let mut uart = crate::UART.lock().unwrap();
            match self.0 {
                0x3fd => if uart.rx.is_some() { 0x61 } else { 0x60 },
                0x3f8 => uart.rx.take().expect("receive register read while empty"),
                0x3fa => if uart.rx.is_some() { 4 } else if uart.thre {
                    uart.thre = false;
                    2
                } else { 1 },
                _ => panic!("unexpected UART register"),
            }
        }
    }
} } }
mod serial {
    pub fn add_serial_byte(byte: u8) { crate::UART.lock().unwrap().delivered.push(byte); }
    pub fn transmitter_interrupt() {
        let mut uart = crate::UART.lock().unwrap();
        if !uart.injected {
            // RX arrives after the empty LSR read, while THRE still asserts
            // INTRPT. Disabling THRE leaves RX asserted, with no new edge.
            assert!(uart.thre);
            uart.rx = Some(b'x');
            uart.injected = true;
        }
        uart.thre = false;
    }
}
mod arch_impl { pub mod x86_64 { pub mod irq {
    pub fn eoi(_: u8) {
        let uart = crate::UART.lock().unwrap();
        assert!(uart.rx.is_none() && !uart.thre, "UART interrupt still asserted at EOI");
    }
} } }
fn serial_interrupt_handler(stack_frame: InterruptStackFrame) {
''' + body + r'''
}
fn main() {
    serial_interrupt_handler(InterruptStackFrame { code_segment: Segment(3) });
    assert_eq!(UART.lock().unwrap().delivered, [b'x']);
}
'''
        with tempfile.TemporaryDirectory(prefix='serial-irq-') as directory:
            src, binary = Path(directory) / 'irq.rs', Path(directory) / 'irq'
            src.write_text(harness)
            built = subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(src), '-o', str(binary)],
                                   capture_output=True, text=True)
            self.assertEqual(built.returncode, 0, built.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
