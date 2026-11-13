#!/usr/bin/env python3
import json
import fcntl
import os
from pathlib import Path
import signal
import runpy
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
HELPER = Path(os.environ.get('QEMU_SMB_HELPER', ROOT / 'scripts/qemu-smb-restart.py'))


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
import os, shlex, sys
parts = shlex.split(sys.argv[-1].split('exec python3 -u -c ', 1)[1])
exec(parts[0])
''')
        incus = self.path / 'incus'
        incus.write_text('''#!/usr/bin/env python3
import fcntl, os, subprocess, sys
from pathlib import Path
args = sys.argv[sys.argv.index('--') + 1:]
if args[0] == 'python3':
    os.execvp(args[0], args)
with (Path(os.environ['BREENIX_METAL_STATE']) / 'lease.lock').open('a') as lock:
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = 'unlocked'
    except BlockingIOError:
        state = 'locked'
with open(os.environ['SSH_LOG'], 'a') as log:
    log.write('action ' + args[1] + ' ' + state + '\\n')
''')
        incus.chmod(0o755)
        sudo = self.path / 'sudo'
        sudo.write_text('#!/bin/sh\nshift\nexec "$@"\n')
        sudo.chmod(0o755)
        site = self.path / 'site'
        import sys
        sys.path.insert(0, str(ROOT / 'scripts'))
        from vm_site import REQUIRED
        site.write_text(''.join(f'{key}=fixture\n' for key in REQUIRED) + 'SMB_TEST_SSH_HOST=fixture\n')
        site.chmod(0o600)
        ssh.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.path) + os.pathsep + os.environ['PATH'], SSH_LOG=str(self.log), BREENIX_VM_SITE=str(site),
                        BREENIX_METAL_STATE=str(self.path), METAL_INSTANCE='metal-fixture', SMB_TEST_INSTANCE='fixture')

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

    def test_admission_stays_locked_until_owner_releases(self):
        code = runpy.run_path(str(HELPER))['LEASE_CHECK'].replace('/var/lib/breenix-metal', str(self.path))
        proc = subprocess.Popen(['python3', '-u', '-c', code], env=self.env,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(proc.stdout.readline().strip(), 'clear')
            with (self.path / 'lease.lock').open('a') as lock:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            proc.communicate('', timeout=5)
        self.assertEqual(proc.returncode, 0)

    def test_boot_holds_machine_admission_through_smb_cleanup(self):
        # A real flock contender models metal claim_lease's admission section.
        marker, release = self.path / 'running', self.path / 'release'
        child = self.path / 'guest.py'
        child.write_text("from pathlib import Path\nimport time\n"
                         f"Path({str(marker)!r}).touch()\n"
                         f"while not Path({str(release)!r}).exists(): time.sleep(.02)\n")
        proc = subprocess.Popen(['python3', str(HELPER), 'run', '--serial', str(self.serial),
                                 '--instance', 'fixture', '--', 'python3', str(child)],
                                env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: proc.poll() is None and proc.kill())
        for _ in range(100):
            if marker.exists() or proc.poll() is not None: break
            time.sleep(.05)
        self.assertTrue(marker.exists(), proc.communicate(timeout=5) if proc.poll() is not None else 'no child')
        with (self.path / 'lease.lock').open('a') as lock:
            with self.assertRaises(BlockingIOError):
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        contender = self.run_helper('check')
        self.assertNotEqual(contender.returncode, 0)
        self.serial.write_text('SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=1\n')
        self.wait_requests(1)
        release.touch()
        stdout, stderr = proc.communicate(timeout=10)
        self.assertEqual(proc.returncode, 0, (stdout, stderr))
        with (self.path / 'lease.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.assertEqual([line for line in self.log.read_text().splitlines()
                          if line.startswith('action ')], ['action stop locked', 'action start locked'])

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
        self.assertEqual(calls, ['action stop locked', 'action start locked'])

    def test_lone_stop_is_restored(self):
        proc = self.watch()
        self.serial.write_text('SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=3\n')
        self.wait_requests(1)
        proc.send_signal(signal.SIGTERM)
        self.assertEqual(proc.wait(timeout=5), 0)
        self.assertEqual(self.log.read_text().splitlines(), ['action stop locked', 'action start locked'])
        self.assertEqual(json.loads(Path(str(self.serial) + '.smb-restart.json').read_text())['cleanupStartExit'], 0)

    def test_authoritative_machine_lease(self):
        code = runpy.run_path(str(HELPER))['LEASE_CHECK']
        code = code.replace('/var/lib/breenix-metal', str(self.path))
        lease = self.path / 'lease.json'
        for contents in (None, '{"id":"fixture","state":"powered"}', 'broken-json'):
            with self.subTest(lease=contents):
                if contents is None:
                    lease.unlink(missing_ok=True)
                else:
                    lease.write_text(contents)
                result = subprocess.run(['python3', '-c', code], input='', capture_output=True, text=True,
                                        env=self.env)
                self.assertEqual(result.returncode == 0, contents is None, result.stderr)
                if contents is None:
                    self.assertEqual(result.stdout.strip(), 'clear')

    def test_live_lease_refused(self):
        (self.path / 'lease.json').write_text('{"id":"active"}')
        result = self.run_helper('check')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('admission', result.stderr.lower())


if __name__ == '__main__':
    unittest.main()
