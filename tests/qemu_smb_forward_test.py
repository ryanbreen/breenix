"""The QEMU SMB forward follows the selected suite and launcher lifetime."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import sys
sys.path[:0] = [str(Path(__file__).resolve().parents[1] / p) for p in ("scripts", "scripts/metal", "tests")]
import metal_smb_test
import metal_root
import vm_site

ROOT = Path(__file__).resolve().parents[1]


class ForwardTest(unittest.TestCase):
    def test_forward_lifecycle(self):
        launcher = (ROOT / 'scripts/boot-interactive.sh').read_text()
        self.assertLess(launcher.index('qemu_smb_forward_start "$MODE"'), launcher.index('scripts/qemu-desktop-stage.py" "$WRITABLE"'))
        self.assertLess(launcher.index('scripts/qemu-desktop-stage.py" "$WRITABLE"'), launcher.index('scripts/watch-qemu.py"'))
        self.assertIn('qemu_smb_forward_stop', launcher)
        for case, fails in [('guskit/smb-save-open', False), ('guskit/smb-save-open', True), ('bterm/prompt', False)]:
            with self.subTest(case=case, fails=fails), tempfile.TemporaryDirectory() as directory, metal_smb_test.SmbStagingTest().gui_root() as (image, _, config):
                tmp = Path(directory)
                site = tmp / 'site'
                values = {key: 'fixture' for key in vm_site.REQUIRED}
                values.update(config, SMB_TEST_HOST='fixture.invalid', SMB_TEST_PORT='445', SMB_TEST_INSTANCE='fixture', SMB_TEST_SSH_HOST='ssh-fixture')
                site.write_text(''.join(f'{key}={value}\n' for key, value in values.items()))
                site.chmod(0o600)
                ssh = tmp / 'ssh'
                ssh.write_text('''#!/usr/bin/env python3
import os, signal, socket, sys
from pathlib import Path
p = Path(os.environ['TEST_DIR'])
(p / 'args').write_text('\\n'.join(sys.argv[1:]))
(p / 'pid').write_text(str(os.getpid()))
def stop(*args):
    (p / 'stopped').write_text('stopped')
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
s = socket.socket()
s.bind(('127.0.0.1', int(sys.argv[sys.argv.index('-L') + 1].split(':')[1])))
s.listen()
while True:
    c, _ = s.accept()
    c.close()
''')
                ssh.chmod(0o755)
                env = dict(os.environ, ROOT=str(ROOT), PATH=f'{tmp}:' + os.environ['PATH'], BREENIX_VM_SITE=str(site), TEST_DIR=str(tmp), IMAGE=str(image), CASE=case, FAIL='1' if fails else '0')
                script = '''set -e
source "$ROOT/scripts/qemu-smb-forward.sh"
trap qemu_smb_forward_stop EXIT
qemu_smb_forward_start suite gui-apps "$CASE"
if [ -n "$SMB_FORWARD_PID" ]; then
  for i in {1..100}; do [ ! -f "$TEST_DIR/pid" ] || break; sleep .01; done
  python3 "$ROOT/scripts/qemu-desktop-stage.py" "$IMAGE" gui-apps "$CASE" --forward-port "$SMB_FORWARD_PORT"
fi
[ "$FAIL" = 0 ] || exit 23
'''
                result = subprocess.run(['bash', '-c', script], env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 23 if fails else 0, result.stderr)
                if case.startswith('guskit/'):
                    debugfs, = metal_root.require_e2fsprogs('debugfs')
                    host, port = subprocess.check_output([debugfs, '-R', 'cat /etc/breenix/smb-test-server', str(image)], stderr=subprocess.DEVNULL, text=True).split()
                    self.assertEqual(host, '10.0.2.2')
                    self.assertIn(f'127.0.0.1:{port}:fixture.invalid:445', (tmp / 'args').read_text())
                    self.assertIn('ssh-fixture', (tmp / 'args').read_text().splitlines())
                    self.assertTrue((tmp / 'stopped').exists())
                else:
                    self.assertFalse((tmp / 'args').exists())


if __name__ == '__main__':
    unittest.main()
