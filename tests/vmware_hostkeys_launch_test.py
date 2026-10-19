"""Exercise the launcher's selected key backend, declaration and VMX settings."""
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
FUNCTIONS = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', (ROOT / 'run.sh').read_text()))


def definitions(*names):
    return '\n'.join(f'{name}() {{\n{FUNCTIONS[name]}\n}}' for name in names)


class VmwareHostkeysLaunchTest(unittest.TestCase):
    def configure(self, suite, mode='suite', override=None):
        env = dict(os.environ, SUITE=suite, BOOT_MODE=mode)
        env.pop('BREENIX_VMWARE_VNC_PORT', None)
        if override is not None:
            env['BREENIX_VMWARE_VNC_PORT'] = override
        return subprocess.run(['bash', '-ec', definitions('configure_vmware_hostkeys') +
                               '\nconfigure_vmware_hostkeys\nprintf "%s" "$VMWARE_VNC_PORT"'],
                              env=env, capture_output=True, text=True, timeout=5)

    def test_console_anywhere_in_list_selects_free_port(self):
        with socket.socket() as occupied:
            occupied.bind(('127.0.0.1', 0))
            for suite in ('console', 'console,smoke', 'smoke,console,terminals', 'smoke,console'):
                with self.subTest(suite=suite):
                    result = self.configure(suite)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    port = int(result.stdout)
                    self.assertNotEqual(port, occupied.getsockname()[1])
                    with socket.socket() as check:
                        check.bind(('127.0.0.1', port))

    def test_unrelated_suites_and_other_modes_leave_vnc_off(self):
        for suite, mode in (('smoke,terminals', 'suite'), ('console-extra', 'suite'),
                            ('', 'default'), ('', 'probe'), ('console', 'program')):
            with self.subTest(suite=suite, mode=mode):
                result = self.configure(suite, mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, '')

    def test_explicit_port_wins_and_invalid_ports_fail(self):
        for suite, mode in (('console', 'suite'), ('smoke,console', 'suite'), ('', 'program')):
            result = self.configure(suite, mode, '5999')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, '5999')
        for invalid in ('0', '65536', '999999', '-1', 'port', '5900\nRemoteDisplay.vnc.ip=bad'):
            result = self.configure('console', override=invalid)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn('needs a port number', result.stderr)

    def test_selected_port_reaches_loopback_vmx_and_guest_declaration(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scripts = root / 'scripts'
            scripts.mkdir()
            (scripts / 'hostkeys.py').symlink_to(ROOT / 'scripts/hostkeys.py')
            (scripts / 'write-resolv-conf.sh').write_text('#!/bin/sh\nexit 0\n')
            (scripts / 'write-boot-target.sh').write_text(
                '#!/bin/sh\nprintf "%s\\n" "$BREENIX_HOSTKEYS" > "$1.declaration"\nprintf "%s\\n" "$@" > "$1.args"\n')
            for script in scripts.glob('*.sh'):
                script.chmod(0o755)
            image = root / 'image'
            image.write_text('fixture disk')
            for override in ('', '5999'):
                env = dict(os.environ, BREENIX_ROOT=str(root), BOOT_MODE='suite', SUITE='smoke,console',
                           ONLY='', PROBE='false', PARALLELS='false', VMWARE='true', VMX_FILE=str(root / 'vm.vmx'),
                           BREENIX_VMWARE_VNC_PORT=override)
                script = definitions('configure_vmware_hostkeys', 'write_vmware_vnc', 'writes_boot_target',
                                     'target_nameserver', 'stage_boot_ext2_disk') + '''
configure_vmware_hostkeys
: > "$VMX_FILE"
write_vmware_vnc
stage_boot_ext2_disk "$1" "$2"
printf '%s' "$VMWARE_VNC_PORT"
'''
                result = subprocess.run(['bash', '-ec', script, 'stage', str(image), str(root / 'staged')],
                                        env=env, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                vmx = (root / 'vm.vmx').read_text()
                self.assertEqual(vmx, 'RemoteDisplay.vnc.enabled = "TRUE"\n'
                                 f'RemoteDisplay.vnc.port = "{result.stdout}"\n'
                                 'RemoteDisplay.vnc.ip = "127.0.0.1"\n')
                self.assertEqual((root / 'staged/ext2-boot-target.img.declaration').read_text().strip(),
                                 subprocess.check_output(['python3', str(ROOT / 'scripts/hostkeys.py'),
                                                          'declare', '--backend', 'vnc:127.0.0.1:' + result.stdout], text=True).strip())
                self.assertIn('smoke,console', (root / 'staged/ext2-boot-target.img.args').read_text())

    def test_watcher_uses_selected_backend_and_records_owned_pid(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'scripts').mkdir()
            (root / 'scripts/hostkeys.py').write_text(
                'import json, os, signal, sys\nfrom pathlib import Path\n'
                'Path(sys.argv[sys.argv.index("--log") + 1]).write_text(json.dumps([os.getpid(), sys.argv[1:]]))\n'
                'signal.pause()\n')
            for parallels, port, expected in (('true', '', 'prlsdk:owned-vm'),
                                               ('false', '5999', 'vnc:127.0.0.1:5999'),
                                               ('false', '', 'none')):
                env = dict(os.environ, BREENIX_ROOT=str(root), BOOT_MODE='suite', PARALLELS=parallels,
                           PARALLELS_VM='owned-vm', VMWARE_VNC_PORT=port, SERIAL_LOG=str(root / 'serial'))
                script = definitions('start_vm_hostkeys') + '''
trap 'kill "$HOSTKEYS_PID"; wait "$HOSTKEYS_PID" 2>/dev/null' EXIT
start_vm_hostkeys
printf '%s\\n' "$HOSTKEYS_PID"
read -r done
'''
                process = subprocess.Popen(['bash', '-c', script], env=env, stdin=subprocess.PIPE,
                                           stdout=subprocess.PIPE, text=True)
                try:
                    pid = int(process.stdout.readline())
                    import json
                    import time
                    deadline = time.monotonic() + 3
                    log = root / 'serial.hostkeys'
                    while not log.exists() and time.monotonic() < deadline:
                        time.sleep(0.02)
                    recorded_pid, args = json.loads(log.read_text())
                    self.assertEqual(pid, recorded_pid)
                    self.assertEqual(args[args.index('--backend') + 1], expected)
                    self.assertEqual(args[args.index('--parent') + 1], str(process.pid))
                finally:
                    process.communicate('done\n', timeout=5)
                    (root / 'serial.hostkeys').unlink(missing_ok=True)
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)


if __name__ == '__main__':
    unittest.main()
