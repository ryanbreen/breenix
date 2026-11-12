#!/usr/bin/env python3
"""Check the A.3 pre-exec boundary without starting a guest."""

import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / 'scripts/a3-proof-gate.py'


def module(path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class ProofGateTests(unittest.TestCase):
    def test_dry_gate_accepts_the_production_wrapper_chain(self):
        self.assertTrue(GATE.is_file(), 'the reusable A.3 proof gate is missing')
        arm = module(ROOT / 'scripts/qemu-arm64.py')
        export = re.search(r'^export BREENIX_QEMU_UI_LAUNCHER="([^"]+)"',
                           (ROOT / 'docker/qemu/lib/qemu-host-lock.sh').read_text(), re.M)
        wrapper = export[1].replace('$_QHL_REPO', str(ROOT))
        qemu = subprocess.check_output([str(ROOT / 'scripts/build-qemu-gl.sh'), '--print-path'],
                                       text=True).strip()
        with tempfile.TemporaryDirectory() as directory:
            serial = str(Path(directory) / 'serial log.txt')
            args = ['-M', 'virt', '-cpu', 'max', '-display', 'none',
                    '-spice', 'unix=on,addr=spice.sock,gl=es',
                    '-device', 'virtio-gpu-gl-device', '-kernel', '/unused/kernel',
                    '-drive', 'if=none,id=ext2,file=/unused/disk image',
                    '-chardev', 'stdio,logfile=' + serial]
            # Exercise main's argv assembly and environment selection. Only the
            # final exec is intercepted.
            with patch.dict(os.environ, BREENIX_QEMU_UI_LAUNCHER=wrapper,
                            BREENIX_QEMU_PRE_EXEC_GATE=str(GATE), BREENIX_ARM64_QEMU_ACCEL='hvf'), \
                    patch.object(sys, 'argv', [str(ROOT / 'scripts/qemu-arm64.py'), qemu, *args]), \
                    patch.object(arm.os, 'execvp') as execute:
                arm.main()
            gate_program, command = execute.call_args.args
            self.assertEqual(gate_program, str(GATE))
            self.assertEqual(command[:3], [str(GATE), wrapper, qemu])
            result = subprocess.run([sys.executable, str(GATE), '--dry-run', *command[1:]],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            record = json.loads(result.stdout)
            self.assertEqual(record['argv'], command[1:])
            self.assertEqual(record['hashes']['qemu'],
                             'e52ce6461a0bad90d7105991d695fd9288485256a568ee0e15ca9f43f3d0c09b')
            native = subprocess.check_output([wrapper], text=True).strip()
            self.assertEqual(record['native_launcher'], native)
            self.assertNotEqual(native, wrapper)
            for replacement in ([native, *command[2:]], [wrapper, '/wrong/qemu', *command[3:]]):
                refused = subprocess.run([sys.executable, str(GATE), '--dry-run', *replacement],
                                         capture_output=True, text=True)
                self.assertNotEqual(refused.returncode, 0, refused.stdout)

    def test_prediction_precedes_unchanged_exec_and_refusal_stops_it(self):
        self.assertTrue(GATE.is_file(), 'the reusable A.3 proof gate is missing')
        gate = module(GATE)
        argv = ['wrapper', 'signed-qemu', '-accel', 'hvf', 'argument with spaces',
                '-D', '/unused/serial.qemu-trace.log', '-pidfile', '/unused/serial.qemu.pid']
        with tempfile.TemporaryDirectory() as directory:
            config = {'output_dir': directory, 'ledger': str(Path(directory) / 'ledger.jsonl'),
                      'head': 'a' * 40, 'coordinator_line': 266}
            Path(config['ledger']).write_text(json.dumps({'role': 'coordinator', 'text': 'fixture'}) + '\n')
            qemu = Path(directory) / 'bundle/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64'
            qemu.parents[3].mkdir()
            (qemu.parents[3] / 'PINS').write_text('fixture pins\n')
            sampler = Mock(pid=12345)
            sampler.poll.return_value = None
            def execute(program, arguments):
                self.assertEqual(program, argv[0])
                self.assertEqual(arguments, argv)
                prediction = json.loads(Path(config['ledger']).read_text().splitlines()[-1])
                self.assertEqual(prediction['type'], 'prediction')
                self.assertIn('25%', prediction['text'])
                self.assertEqual(json.loads((Path(directory) / 'artifact-gate.json').read_text())['argv'], argv)
            with patch.dict(os.environ, BREENIX_QEMU_CASE_SAMPLE_SERIAL='/unused/serial'), \
                    patch.object(gate, 'check_chain', return_value={'hashes': {}, 'native_launcher': 'native', 'qemu': str(qemu)}), \
                    patch.object(gate, 'check_artifacts', return_value={'suite': 'sha'}) as artifacts, \
                    patch.object(gate, 'check_head') as head, \
                    patch.object(gate, 'check_warnings') as warnings, \
                    patch.object(gate, 'check_coordinator') as coordinator, \
                    patch.object(gate, 'start_sampler', return_value=sampler), \
                    patch.object(gate, 'wait_for_load', return_value={'load1': 8.0, 'waited_s': 2.0,
                                                                   'load_timeout': False}), \
                    patch.object(gate.os, 'execvp', side_effect=execute) as execvp:
                gate.run(argv, config)
                artifacts.assert_called_once()
                warnings.assert_called_once()
                self.assertGreaterEqual(head.call_count, 1)
                self.assertGreaterEqual(coordinator.call_count, 1)
                execvp.assert_called_once()
                execvp.reset_mock()
                artifacts.side_effect = ValueError('boot disk suite differs')
                with self.assertRaisesRegex(ValueError, 'boot disk suite differs'):
                    gate.run(argv, config)
                execvp.assert_not_called()

    def test_load_gate_waits_below_nine_or_reports_the_thirty_minute_limit(self):
        self.assertTrue(GATE.is_file(), 'the reusable A.3 proof gate is missing')
        gate = module(GATE)
        sampler = Mock()
        sampler.poll.return_value = None
        with patch.object(gate, 'read_load', side_effect=[(10.0, 'high'), (8.0, 'low')]), \
                patch.object(gate.time, 'monotonic', side_effect=[0.0, 0.0, 2.0]), \
                patch.object(gate.time, 'sleep') as sleep:
            result = gate.wait_for_load(sampler)
            self.assertEqual(result, {'load1': 8.0, 'waited_s': 2.0, 'load_timeout': False})
            sleep.assert_called_once_with(2)
        with patch.object(gate, 'read_load', return_value=(10.0, 'high')), \
                patch.object(gate.time, 'monotonic', side_effect=[0.0, 1800.0]):
            self.assertEqual(gate.wait_for_load(sampler),
                             {'load1': 10.0, 'waited_s': 1800.0, 'load_timeout': True})


if __name__ == '__main__':
    unittest.main()
