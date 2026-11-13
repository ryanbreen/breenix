"""The launcher exposes collaboration only on the requested host port."""
import json
import os
from pathlib import Path
import socket
import subprocess
import unittest

import boot_interactive_qmp_test


class CollabHostTest(unittest.TestCase):
    def setUp(self):
        fixture = boot_interactive_qmp_test.BootInteractiveQmpTest()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.root, self.home, self.calls = fixture.root, fixture.home, fixture.calls
        self.executable = fixture.executable
        scripts = self.root / 'scripts'
        (scripts / 'qemu-smb-forward.sh').write_text('qemu_smb_forward_stop() { :; }\n')
        self.executable(scripts / 'fake-qemu', '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
Path(os.environ['CAPTURE']).write_text(json.dumps(sys.argv[1:]))
''')
        (scripts / 'qemu-arm64.py').write_text('''import os, sys
if '--print-profile' in sys.argv:
 print('default')
else:
 os.execvp(sys.argv[1], sys.argv[1:])
''')
        (scripts / 'watch-qemu.py').write_text('''import os, sys
args = sys.argv[sys.argv.index('--') + 1:]
os.execvp(args[0], args)
''')

    def run_launcher(self, *args):
        env = dict(os.environ, HOME=str(self.home), STUB_VIGIL_ID='', STUB_BOOT_HOME='',
                   CALLS=str(self.calls), CAPTURE=str(self.root / 'args.json'),
                   PATH=f'{self.root / "scripts"}:' + os.environ['PATH'])
        self.executable(self.root / 'scripts/qemu-system-aarch64',
                        '#!/bin/sh\nexec "$(dirname "$0")/fake-qemu" "$@"\n')
        return subprocess.run(['bash', str(self.root / 'scripts/boot-interactive.sh'),
                               '--no-build', '--no-display', '--serial-log', str(self.root / 'serial'),
                               *args], env=env, capture_output=True, text=True, timeout=10)

    def test_forward_default_and_explicit_port(self):
        for mode, port, option in [(['--mode', 'desktop'], 7890, []),
                                   (['--mode', 'program', '--program', '/sbin/probe'], 17890, ['17890'])]:
            with self.subTest(port=port):
                result = self.run_launcher(*mode, '--collab-host', *option)
                self.assertEqual(result.returncode, 0, result.stderr)
                argv = json.loads((self.root / 'args.json').read_text())
                self.assertEqual(argv[argv.index('-netdev') + 1],
                                 f'user,id=net0,hostfwd=tcp:0.0.0.0:{port}-:7890')

    def test_refusals(self):
        for args, message in [(['--mode', 'probe', '--collab-host'], 'mode'),
                              (['--mode', 'desktop', '--collab-host', '0'], 'port'),
                              (['--mode', 'desktop', '--collab-host', '65536'], 'port'),
                              (['--mode', 'desktop', '--collab-host', 'bad'], 'port')]:
            with self.subTest(args=args):
                result = self.run_launcher(*args)
                self.assertEqual(result.returncode, 2)
                self.assertIn(message, result.stderr.lower())
        with socket.socket() as listener:
            listener.bind(('0.0.0.0', 0))
            listener.listen()
            result = self.run_launcher('--mode', 'desktop', '--collab-host', str(listener.getsockname()[1]))
            self.assertEqual(result.returncode, 2)
            self.assertIn('port', result.stderr.lower())


if __name__ == '__main__':
    unittest.main()
