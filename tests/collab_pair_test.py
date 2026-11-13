import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PAIR = ROOT / 'scripts/collab-pair.py'


class CollabPairTest(unittest.TestCase):
    def test_pair_verdict_and_lock(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            launcher = base / 'launcher.py'
            launcher.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys, time
args = sys.argv[1:]
role = 'host' if '--collab-host' in args else 'join'
serial = pathlib.Path(args[args.index('--serial-log') + 1])
serial.parent.mkdir(parents=True, exist_ok=True)
locks = pathlib.Path(os.environ['TEST_DIR']) / 'children'
with locks.open('a') as f: f.write(role + ' ' + os.environ['BREENIX_QEMU_LOCK'] + '\\n')
if os.environ['SCENARIO'] == 'missing' and role == 'join': sys.exit(1)
record = pathlib.Path(os.environ['VIGIL_TEST_HOME']) / 'Library/Application Support/Vigil/breenix/boots' / ('id-' + role)
record.mkdir(parents=True)
(record / 'run.json').write_text(json.dumps({'serial': str(serial), 'id': 'id-' + role}))
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
