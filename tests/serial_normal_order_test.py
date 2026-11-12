"""Live UART owners keep exact records; stalled owners leave a terminated line."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class NormalOrder(unittest.TestCase):
    def test_progress_and_stall_wires(self):
        harness = (ROOT / 'tests/uart_owner.rs').read_text()
        for name in ['stage_lines', 'uart_owner']:
            harness = harness.replace('../kernel/src/' + name + '.rs',
                                      str(ROOT / 'kernel/src' / (name + '.rs')))
        harness += r'''
static ORDER_STATE: State = State::new();
static ORDER_TICKS: AtomicUsize = AtomicUsize::new(0);
static ORDER_MODE: AtomicUsize = AtomicUsize::new(0);
const REQUEST: &[u8] = b"SUITE window-manager HOSTPOINTER focus/ctrl-c seq=409 x=84 y=164 width=2560 height=1600 buttons=0 wheel=0\n";
const EXIT: &[u8] = b"[signal] Process 381 terminated by signal 9\n";
const PREFIX: usize = 11;
thread_local! {
    static RETURNING: std::cell::RefCell<Option<(Ownership<OrderedTest>, usize)>> =
        const { std::cell::RefCell::new(None) };
}
enum OrderedTest {}
impl Port for OrderedTest {
    type Writer = u8;
    fn state() -> &'static State { &ORDER_STATE }
    fn cpu_tag() -> u32 { Test::cpu_tag() }
    fn counter() -> u64 {
        let step = ORDER_TICKS.fetch_add(1, Ordering::Relaxed);
        assert!(step < 10000, "no waiter may spin forever");
        // The waiter advances a scripted millisecond clock. Interleave the
        // existing owner's production writes, without host scheduling or sleeps.
        let mode = ORDER_MODE.load(Ordering::Relaxed);
        if (mode == 1 && step % 20 == 19) || (mode == 2 && step == 49) {
            RETURNING.with(|slot| {
                let mut returning = slot.borrow_mut();
                if let Some((owner, at)) = returning.as_mut() {
                    let end = if mode == 1 { *at + 1 } else { REQUEST.len() };
                    uart_owner::write::<OrderedTest, false>(
                        1, &REQUEST[*at..end], Kind::Plain, owner);
                    *at = end;
                    if end == REQUEST.len() { *returning = None; }
                }
            });
        }
        step as u64 * 1_000_000
    }
    fn ticks(ns: u64) -> u64 { ns }
    fn mask_interrupts() -> u64 { 0 }
    fn restore_interrupts(_: u64) {}
    fn ready() -> bool { true }
    fn write(byte: u8, _: &Ownership<Self>) { WIRE.lock().unwrap().push(byte); }
    fn capture(byte: u8) { Test::capture(byte) }
    fn lead(writer: u8, unit: &[u8]) -> &'static [u8] { Test::lead(writer, unit) }
    fn line_open() -> bool { Test::line_open() }
    fn wrote(writer: u8, byte: u8) { Test::wrote(writer, byte) }
    fn markers() -> &'static [&'static str] { Test::markers() }
}

#[test]
fn progressing_or_returning_owner_preserves_the_exact_wire() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in [1, 2] {
        WIRE.lock().unwrap().clear();
        OPEN.store(0, Ordering::Relaxed);
        ORDER_TICKS.store(0, Ordering::Relaxed);
        ORDER_MODE.store(0, Ordering::Relaxed);
        let first = thread::spawn(|| {
            let mut owner = Ownership::<OrderedTest>::acquire_ordered();
            uart_owner::write::<OrderedTest, false>(
                1, &REQUEST[..PREFIX], Kind::Line, &mut owner);
            owner
        }).join().unwrap();
        // A different CPU waits while the first progresses every 20 ms, or
        // returns after 50 ms. The long record takes much more than STALL_NS.
        RETURNING.with(|slot| *slot.borrow_mut() = Some((first, PREFIX)));
        ORDER_MODE.store(mode, Ordering::Relaxed);
        uart_owner::write::<OrderedTest, false>(
            2, EXIT, Kind::Line, &mut Ownership::acquire_ordered());
        RETURNING.with(|slot| assert!(slot.borrow().is_none()));
        if mode == 1 {
            assert!(ORDER_TICKS.load(Ordering::Relaxed) as u64 * 1_000_000
                > uart_owner::STALL_NS);
        }
        let mut expected = REQUEST.to_vec();
        expected.extend_from_slice(EXIT);
        assert_eq!(*WIRE.lock().unwrap(), expected,
            "a live owner's record was truncated or replayed, mode={mode}");
    }
}

#[test]
fn never_progressing_owner_leaves_one_terminated_line_and_intact_records() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dead = thread::spawn(|| {
        let mut owner = Ownership::<OrderedTest>::acquire_ordered();
        uart_owner::write::<OrderedTest, false>(
            1, &REQUEST[..PREFIX], Kind::Line, &mut owner);
        owner
    }).join().unwrap();
    core::mem::forget(dead);
    for record in [EXIT, REQUEST] {
        uart_owner::write::<OrderedTest, false>(
            2, record, Kind::Line, &mut Ownership::acquire_ordered());
    }
    let mut expected = REQUEST[..PREFIX].to_vec();
    expected.extend_from_slice(b"\r\n");
    expected.extend_from_slice(EXIT);
    expected.extend_from_slice(REQUEST);
    assert_eq!(*WIRE.lock().unwrap(), expected,
        "takeover must terminate the abandoned prefix before intact records");
}
'''
        with tempfile.TemporaryDirectory(prefix='serial-normal-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(harness)
            subprocess.run([
                'taskpolicy', '-b', os.environ.get('RUSTC', 'rustc'),
                '--edition=2021', '--test', '-Dwarnings', str(path / 'check.rs'),
                '-o', str(path / 'check'),
            ], check=True)
            # Separate processes give the two scripts fresh static UART state.
            for case in [
                'progressing_or_returning_owner_preserves_the_exact_wire',
                'never_progressing_owner_leaves_one_terminated_line_and_intact_records',
            ]:
                subprocess.run([str(path / 'check'), '--exact', case], check=True,
                               timeout=10)


if __name__ == '__main__':
    unittest.main()
