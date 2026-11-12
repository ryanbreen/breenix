"""Exercise the local QEMU evidence route and receiver."""
import hashlib
import http.client
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/metal'))
from metal_root import require_e2fsprogs

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/qemu-evidence.py'


class QemuEvidenceTest(unittest.TestCase):
    def test_route_staging_receiver_and_parent_lifetime(self):
        mkfs, debugfs = require_e2fsprogs('mke2fs', 'debugfs')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tree = root / 'tree/etc/breenix'
            tree.mkdir(parents=True)
            (tree / 'boot-target').write_text('suite ui-testing\n')
            image = root / 'disk.img'
            subprocess.run([mkfs, '-q', '-t', 'ext2', '-d', str(root / 'tree'), '-F', str(image), '4096'], check=True, capture_output=True)
            parent = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
            server = subprocess.Popen([sys.executable, str(SCRIPT), 'serve', '--dir', str(root / 'evidence'), '--parent', str(parent.pid)], stderr=subprocess.PIPE)
            try:
                route_file = root / 'evidence/route.json'
                deadline = time.monotonic() + 10
                while not route_file.exists() and time.monotonic() < deadline and server.poll() is None:
                    time.sleep(.05)
                route = json.loads(route_file.read_text())
                self.assertRegex(route['token'], r'^[0-9a-f]{64}$')
                def stage():
                    return subprocess.run([sys.executable, str(SCRIPT), 'stage', str(image), str(root / 'evidence')], capture_output=True)
                self.assertEqual(stage().returncode, 0)
                def read(command):
                    return subprocess.run([debugfs, '-R', command, str(image)], check=True, capture_output=True).stdout.decode()
                self.assertEqual(read('cat /etc/breenix/evidence-route'), f"http://10.0.2.2:{route['port']}/gt710-upload/{route['token']}\n")
                stat = read('stat /etc/breenix/evidence-route')
                self.assertIn('Mode:  0400', stat)
                self.assertRegex(stat, r'User:\s+0\s+Group:\s+0')
                conn = http.client.HTTPConnection('127.0.0.1', route['port'], timeout=3)
                path = '/gt710-upload/' + route['token']
                conn.request('HEAD', path)
                response = conn.getresponse()
                self.assertEqual(response.status, 204)
                self.assertTrue(response.getheader('X-Breenix-Boot'))
                response.read()
                body = b'evidence'
                conn.request('PUT', path, body, {'X-Content-SHA256': '0' * 64})
                self.assertEqual(conn.getresponse().status, 400)
                conn.close()
                conn = http.client.HTTPConnection('127.0.0.1', route['port'], timeout=3)
                conn.request('PUT', path, body, {'X-Content-SHA256': hashlib.sha256(body).hexdigest()})
                self.assertEqual(conn.getresponse().status, 200)
                conn.close()
                self.assertEqual((root / 'evidence/gt710-pixels.rle').read_bytes(), body)
                for target in ('window-manager', 'gui-apps', 'gpu', 'breengel', 'console'):
                    source = root / 'target'
                    source.write_text('suite ' + target + '\n')
                    subprocess.run([debugfs, '-w', '-R', 'rm /etc/breenix/boot-target', str(image)], check=True, capture_output=True)
                    subprocess.run([debugfs, '-w', '-R', f'write {source} /etc/breenix/boot-target', str(image)], check=True, capture_output=True)
                    self.assertNotEqual(stage().returncode, 0, target)
                source.write_text('suite ui-testing\n')
                subprocess.run([debugfs, '-w', '-R', 'rm /etc/breenix/boot-target', str(image)], check=True, capture_output=True)
                subprocess.run([debugfs, '-w', '-R', f'write {source} /etc/breenix/boot-target', str(image)], check=True, capture_output=True)
                subprocess.run([debugfs, '-w', '-R', f'write {source} /etc/breenix/suite-sequence', str(image)], check=True, capture_output=True)
                self.assertNotEqual(stage().returncode, 0)
                parent.terminate()
                parent.wait(timeout=5)
                self.assertEqual(server.wait(timeout=5), 0)
            finally:
                if parent.poll() is None:
                    parent.terminate()
                    parent.wait()
                if server.poll() is None:
                    server.terminate()
                    server.wait()
                server.stderr.close()


if __name__ == '__main__':
    unittest.main()
