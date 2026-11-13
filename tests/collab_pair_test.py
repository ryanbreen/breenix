import json
import os
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch
import unittest

ROOT = Path(__file__).resolve().parents[1]
PAIR = ROOT / 'scripts/collab-pair.py'


class CollabPairTest(unittest.TestCase):
    def test_counted_forward_binds_loopback(self):
        from qemu_collab_host_test import CollabHostTest
        fixture = CollabHostTest()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        with patch.dict(os.environ, BREENIX_COLLAB_HOST_BIND='127.0.0.1'):
            result = fixture.run_launcher('--mode', 'program', '--program',
                                          '/usr/libexec/breenix/collab-lan-host', '--collab-host')
        self.assertEqual(result.returncode, 0, result.stderr)
        argv = json.loads((fixture.root / 'args.json').read_text())
        self.assertEqual(argv[argv.index('-netdev') + 1],
                         'user,id=net0,hostfwd=tcp:127.0.0.1:7890-:7890')

    def test_pair_holds_lease_after_one_child_releases(self):
        import importlib.util
        import socket
        import threading
        spec = importlib.util.spec_from_file_location('collab_pair', PAIR)
        pair = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(pair)
        requests = []
        with tempfile.TemporaryDirectory(dir='/tmp') as tmp, socket.socket(socket.AF_UNIX) as upstream:
            address = str(Path(tmp) / 'upstream.sock')
            upstream.bind(address)
            upstream.listen()

            def supervise():
                for _ in range(2):
                    client, _ = upstream.accept()
                    with client, client.makefile('rb') as stream:
                        requests.append(json.loads(stream.readline()))
                        client.sendall(b'{}\n')

            worker = threading.Thread(target=supervise, daemon=True)
            worker.start()
            with pair.PairSlots(address) as session:
                def request(operation, **values):
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(3)
                        client.connect(session)
                        client.sendall(json.dumps(dict(operation=operation, **values)).encode() + b'\n')
                        with client.makefile('rb') as stream:
                            self.assertEqual(json.loads(stream.readline()), {})

                for role in ('host', 'join'):
                    request('acquire', resource='mac-boot')
                    request('serial', serial=role)
                    request('release', resource='mac-boot')
            worker.join(timeout=3)
            self.assertFalse(worker.is_alive())
            self.assertEqual(requests, [{'operation': 'serial', 'serial': role}
                                        for role in ('host', 'join')])

    def test_pair_verdict_and_lock(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            launcher = base / 'launcher.py'
            launcher.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys, time
args = sys.argv[1:]
role = 'host' if '--collab-host' in args else 'join'
assert args[args.index('--mode') + 1] == 'program'
assert args[args.index('--program') + 1] == '/usr/libexec/breenix/collab-lan-' + role
assert '--no-display' in args and '--no-build' in args
assert os.environ['BREENIX_COLLAB_HOST_BIND'] == '127.0.0.1'
if role == 'host': assert args[args.index('--collab-host') + 1] == '7890'
serial = pathlib.Path(args[args.index('--serial-log') + 1])
serial.parent.mkdir(parents=True, exist_ok=True)
ready = pathlib.Path(os.environ['TEST_DIR'])
(ready / (role + '.ready')).touch()
deadline = time.monotonic() + 3
while not (ready / (('join' if role == 'host' else 'host') + '.ready')).exists():
    assert time.monotonic() < deadline, 'guests were serialized'
    time.sleep(0.01)
locks = pathlib.Path(os.environ['TEST_DIR']) / 'children'
with locks.open('a') as f: f.write(role + ' ' + os.environ['BREENIX_QEMU_LOCK'] + '\\n')
if os.environ['SCENARIO'] == 'missing' and role == 'join': sys.exit(1)
record = pathlib.Path(os.environ['VIGIL_TEST_HOME']) / 'Library/Application Support/Vigil/breenix/boots' / ('id-' + role)
record.mkdir(parents=True)
(record / 'run.json').write_text(json.dumps({'serial': str(serial), 'id': 'id-' + role}))
(record / 'pid').write_text(os.environ['BREENIX_LAUNCHER_PID'])
(record / 'serial.txt').symlink_to(serial)
if os.environ['SCENARIO'] == 'join_fail' and role == 'join':
    line = 'COLLAB_LAN role=join FAIL reason=refused'
else:
    digest = ('b' if os.environ['SCENARIO'] == 'mismatch' and role == 'join' else 'a') * 64
    line = 'COLLAB_LAN role=' + role + ' ops=8 digest=' + digest
serial.write_text(line + '\\n')
time.sleep(0.1)
''')
            launcher.chmod(0o755)
            lock = base / 'lock.sh'
            lock.write_text('qemu_host_lock_acquire() { echo acquire >> "$TEST_DIR/acquires"; }\nqemu_host_lock_release() { :; }\n')
            for scenario in ('pass', 'mismatch', 'join_fail', 'missing'):
                with self.subTest(scenario=scenario):
                    work = base / scenario
                    work.mkdir()
                    env = dict(os.environ, TEST_DIR=str(work), SCENARIO=scenario,
                               VIGIL_TEST_HOME=str(work), BREENIX_SLOT_SESSION='test')
                    result = subprocess.run(['python3', str(PAIR), '--output', str(work / 'verdict.json'),
                                             '--launcher', str(launcher), '--lock-library', str(lock)],
                                            env=env, capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode == 0, scenario == 'pass', result.stderr)
                    verdict = json.loads((work / 'verdict.json').read_text())
                    self.assertEqual(verdict['verdict'], 'PASS' if scenario == 'pass' else 'FAIL')
                    self.assertEqual((work / 'acquires').read_text().splitlines(), ['acquire'])
                    children = (work / 'children').read_text().splitlines()
                    self.assertEqual(len(children), 2)
                    locks = [line.split(' ', 1)[1] for line in children]
                    self.assertEqual(len(set(locks)), 2)
                    self.assertTrue(all(value.startswith('/') and value != 'off' for value in locks))
                    if scenario == 'pass':
                        for role in ('host', 'join'):
                            self.assertEqual(verdict[role]['vigil_id'], 'id-' + role)
                            self.assertTrue(Path(verdict[role]['serial']).is_file())


if __name__ == '__main__':
    unittest.main()
