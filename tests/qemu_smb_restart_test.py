#!/usr/bin/env python3
import json
import os
from pathlib import Path
import signal
import runpy
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / 'scripts/qemu-smb-restart.py'


class RestartTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name)
        self.serial = self.path / 'serial'
        self.serial.touch()
        self.log = self.path / 'ssh.log'
        ssh = self.path / 'ssh'
        ssh.write_text('''#!/usr/bin/env python3
import os, sys
with open(os.environ['SSH_LOG'], 'a') as f: f.write(' '.join(sys.argv[1:]) + '\\n')
if 'python3' in sys.argv: print(os.environ.get('LEASE', 'clear'))
''')
        ssh.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.path) + os.pathsep + os.environ['PATH'], SSH_LOG=str(self.log))

    def run_helper(self, *args):
        return subprocess.run(['python3', str(HELPER), *args], env=self.env, capture_output=True, text=True)

    def watch(self):
        proc = subprocess.Popen(['python3', str(HELPER), 'watch', '--serial', str(self.serial), '--instance', 'fixture'], env=self.env)
        self.addCleanup(lambda: proc.poll() is None and proc.kill())
        return proc

    def wait_requests(self, count):
        record = Path(str(self.serial) + '.smb-restart.json')
        for _ in range(100):
            if record.exists():
                data = json.loads(record.read_text())
                if len(data['requests']) >= count and all('exit' in x for x in data['requests']):
                    return data
            time.sleep(.05)
        self.fail('helper did not process requests')

    def test_requests_and_cleanup(self):
        proc = self.watch()
        with self.serial.open('a') as f:
            f.write('SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=1\n' * 2)
            f.write('SUITE other SMBSTOP guskit/smb-share-gone seq=2\n')
            f.write('SUITE gui-apps SMBSTART guskit/smb-share-gone seq=2\n')
        self.wait_requests(2)
        proc.send_signal(signal.SIGTERM)
        self.assertEqual(proc.wait(timeout=5), 0)
        calls = self.log.read_text().splitlines()
        self.assertEqual([line.split('systemctl ')[1] for line in calls], ['stop smbd', 'start smbd'])
        self.assertTrue(all('incus exec fixture -- systemctl' in line for line in calls))

    def test_lone_stop_is_restored(self):
        proc = self.watch()
        self.serial.write_text('SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=3\n')
        self.wait_requests(1)
        proc.send_signal(signal.SIGTERM)
        self.assertEqual(proc.wait(timeout=5), 0)
        self.assertEqual([line.split('systemctl ')[1] for line in self.log.read_text().splitlines()], ['stop smbd', 'start smbd'])
        self.assertEqual(json.loads(Path(str(self.serial) + '.smb-restart.json').read_text())['cleanupStartExit'], 0)

    def test_authoritative_machine_lease(self):
        code = runpy.run_path(str(HELPER))['LEASE_CHECK']
        code = code.replace('/var/lib/breenix-metal', str(self.path))
        lease = self.path / 'lease.json'
        for contents, expected in ((None, 'clear'), ('{"id":"fixture","state":"powered"}', 'claimed'),
                                   ('broken-json', 'unknown')):
            with self.subTest(lease=contents):
                if contents is None:
                    lease.unlink(missing_ok=True)
                else:
                    lease.write_text(contents)
                result = subprocess.run(['python3', '-c', code], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)

    def test_live_lease_refused(self):
        self.env['LEASE'] = 'claimed'
        result = self.run_helper('check')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('lease', result.stderr.lower())


if __name__ == '__main__':
    unittest.main()
