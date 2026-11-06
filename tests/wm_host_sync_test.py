"""W14: run the production parking and text cases with a virtual host clock.

Only transport, client storage, scanout and the clock are fixtures. The case
bodies, deadline loop and ACK reader run from the production sources.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class HostSync(unittest.TestCase):
    def test_delayed_park_and_key_resend(self):
        cases = (ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text()
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/wm_host_sync.rs').read_text()
        helpers = '\n'.join(item(suite, marker) for marker in [
            'pub fn hostkeys_acked(', 'pub fn hostpointer_acked(',
            'fn wait_host_ack(', 'fn host_ack_time(',
        ] if marker in suite)
        deadline = item(desktop, 'pub fn until(')
        if 'pub fn until_from(' in desktop:
            deadline += '\n' + item(desktop, 'pub fn until_from(')
        program = fixture.replace('// HELPERS', helpers).replace('// DEADLINE', deadline)
        program = program.replace('// PARKED', item(cases, 'fn parked('))
        text_case = item(cases, 'pub fn text_exact(')
        program = program.replace('// TEXT_EXACT', text_case)
        if 'hostkeys_acked(' in text_case:
            program = program.replace('use breenix_suite::suite::hostkeys;\n', '')
        with tempfile.TemporaryDirectory(prefix='wm-host-sync-') as directory:
            temp = Path(directory)
            # Only redirect the proc file boundary. Parsing and polling stay real.
            program = program.replace('/proc/kmsg', str(temp / 'kmsg'))
            (temp / 'check.rs').write_text(program)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + ['--test', str(temp / 'check.rs'), '-o', str(temp / 'check')], check=True)
            subprocess.run([str(temp / 'check'), '--test-threads=1', '--nocapture'], check=True)

    def test_tagged_packets_keep_their_ticket_through_retry_and_guest_ack(self):
        import ast
        import importlib.util
        import zlib
        spec = importlib.util.spec_from_file_location('hostkeys', ROOT / 'scripts/hostkeys.py')
        hostkeys = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(hostkeys)
        tree = ast.parse((ROOT / 'scripts/metal/metal-boot').read_text())
        sender = next(node for node in ast.walk(tree) if isinstance(node, ast.FunctionDef) and node.name == 'replay_send')
        namespace = {'zlib': zlib}
        exec(compile(ast.Module(body=[sender], type_ignores=[]), 'metal-boot:replay_send', 'exec'), namespace)

        class Driver:
            replay_send = namespace['replay_send']

            def __init__(self):
                self.wire, self.retries, self.attempts = [], [], {}

            def console_send(self, line, paced):
                self.wire.append(line)

            def wait_replay_ack(self, sequence, kind='BXKEY'):
                identity = kind, sequence
                self.attempts[identity] = self.attempts.get(identity, 0) + 1
                return identity != ('BXKEY', 9) or self.attempts[identity] > 1

            def event(self, kind, **fields):
                self.retries.append((kind, fields))

        driver = Driver()
        watcher = hostkeys.Watcher(hostkeys.ReplayKeys(driver.replay_send, pace=lambda _: None),
                                  'replay', log=lambda _: None)
        watcher.line('SUITE window-manager HOSTKEY focus/text-exact seq=7 '
                     'keys=shift-h,e,l,l,o,space,shift-w,o,r,l,d ticket=55')
        watcher.line('SUITE window-manager HOSTPOINTER lifecycle/close-request seq=8 '
                     'x=2520 y=1500 width=2560 height=1600 buttons=0 wheel=0 ticket=56')
        self.assertEqual(driver.retries, [('hostkey-retry', {'sequence': 9, 'attempt': 1})])
        self.assertEqual(driver.wire[8], driver.wire[9])
        self.assertEqual([i for i, line in enumerate(driver.wire) if 'ticket=' in line], [11, 12])
        # Exercise the fixed ingress buffer with a numbered, maximally sized line.
        driver.replay_send('BXKEY ' + ','.join(['0000000000000000'] * 16) + ' ticket=18446744073709551615')
        runtime = (ROOT / 'kernel/src/input/host/runtime.rs').read_text()
        emission = item(runtime[runtime.index('// Serial output is deliberately'):], '    if let Some(ack) = ack {')
        with tempfile.TemporaryDirectory(prefix='wm-wire-ack-') as directory:
            temp = Path(directory)
            (temp / 'wire').write_text('\n'.join(driver.wire) + '\n')
            program = r'''use std::sync::Mutex;
#[path = "HOST_PATH"] pub mod host;
static SERIAL: Mutex<Vec<String>> = Mutex::new(Vec::new());
static KMSG: Mutex<Vec<String>> = Mutex::new(Vec::new());
#[macro_export] macro_rules! serial_println { ($($arg:tt)*) => { crate::SERIAL.lock().unwrap().push(format!($($arg)*)); } }
#[macro_export] macro_rules! diag_println { ($($arg:tt)*) => { crate::KMSG.lock().unwrap().push(format!($($arg)*)); } }
mod window { pub mod events { pub fn now_ns() -> u64 { 2150000000 } } }
fn emit(ack: host::Ack) { let ack = Some(ack); EMISSION }
#[test] fn watcher_wire_guest_ack_round_trip() {
    let mut channel = host::Channel::new();
    let mut lines = host::SerialLines::new();
    let mut received = 0;
    let mut duplicates = 0;
    for wire in std::fs::read_to_string("WIRE_PATH").unwrap().lines() {
        lines.feed_bytes(wire.as_bytes(), &mut |_| {});
        // SerialLines has a byte ingress API; feed the line including LF below.
        received += 1;
        let receipt = channel.accept(wire.as_bytes(), (2560,1600)).unwrap();
        if receipt.added == 0 { duplicates += 1; }
        let ack = receipt.ack.unwrap();
        let ticket = wire.split(" ticket=").nth(1).map(|s| s.parse::<u64>().unwrap());
        assert_eq!(ack.ticket, ticket);
        emit(ack);
        while channel.pop().is_some() {}
    }
    assert_eq!((received, duplicates), (14,1));
    let records = KMSG.lock().unwrap();
    assert_eq!(records.as_slice(), [
        "HOSTINPUT_ACK BXKEY_ACK 000000000000000b ticket=55 ns=2150000000",
        "HOSTINPUT_ACK BXPTR_ACK 0000000000000001 ticket=56 ns=2150000000",
        "HOSTINPUT_ACK BXKEY_ACK 000000000000000c ticket=18446744073709551615 ns=2150000000",
    ]);
    let mut bad = std::fs::read_to_string("WIRE_PATH").unwrap().lines().last().unwrap().as_bytes().to_vec();
    *bad.last_mut().unwrap() = b'4';
    assert!(channel.accept(&bad, (2560,1600)).is_err(), "ticket is covered by the CRC");
}
'''
            # Feed the actual bounded byte ingress, then validate its emitted line.
            program = program.replace('        lines.feed_bytes(wire.as_bytes(), &mut |_| {});',
                '        let mut framed = Vec::new();\n        for byte in wire.bytes().chain([10]) { lines.feed(byte, |line| framed.push(line.to_vec())); }\n        assert_eq!(framed, vec![wire.as_bytes().to_vec()]);')
            program = program.replace('HOST_PATH', str(ROOT / 'kernel/src/input/host.rs'))
            program = program.replace('WIRE_PATH', str(temp / 'wire')).replace('EMISSION', emission)
            (temp / 'check.rs').write_text(program)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + ['--test', str(temp / 'check.rs'), '-o', str(temp / 'check')], check=True)
            subprocess.run([str(temp / 'check'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
