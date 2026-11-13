#!/usr/bin/env python3
"""Stage init's GUI suite selection and site time servers on a writable QEMU root."""
import argparse
from pathlib import Path
import re
import subprocess
import tempfile

from metal.metal_root import require_e2fsprogs, smb_gui_selection
from vm_site import load_site


def stage(image, suite, case):
    if not smb_gui_selection('suite', suite, case or None):
        return
    site = load_site()
    keys = ('SMB_TEST_USER', 'SMB_TEST_PASSWORD', 'SMB_TEST_HOST', 'SMB_TEST_PORT', 'SMB_TEST_INSTANCE')
    if any(not site.get(key) for key in keys):
        raise ValueError('GUI SMB staging needs SMB_TEST_USER, SMB_TEST_PASSWORD, SMB_TEST_HOST, SMB_TEST_PORT and SMB_TEST_INSTANCE in the VM site')
    user, password, host, port, instance = (site[key] for key in keys)
    if (any(c in user + password for c in '\r\n\x00')
            or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.-]{0,252}', host)
            or not port.isdecimal() or not 1 <= int(port) <= 65535
            or any(c in instance for c in '\r\n\x00')):
        raise ValueError('invalid private GUI SMB configuration')
    servers = site['SNTP_SERVERS'].split()
    if len(servers) > 8 or any(not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?', name) for name in servers):
        raise ValueError('SNTP_SERVERS needs up to eight DNS names or IPv4 addresses')
    debugfs, = require_e2fsprogs('debugfs')
    image = Path(image).resolve()
    with tempfile.TemporaryDirectory(prefix='qemu-desktop-') as tmp:
        tmp = Path(tmp)
        files = {
            '/etc/breenix/desktop-suite': ('gui-apps\n', '0644'),
            '/etc/breenix/sntp-servers': (''.join(name + '\n' for name in servers), '0644'),
            '/etc/breenix/smb-test.cred': (f'username={user}\npassword={password}\n', '0600'),
            '/etc/breenix/smb-test-server': (f'{host} {port}\n', '0600'),
            '/etc/shares': (''.join(
                f'//{host}/{share} /mnt/guskit-{share} smb credfile=/etc/breenix/smb-test.cred,port={port}\n'
                for share in ('rw', 'ro')), '0644'),
        }
        commands = ['mkdir /mnt']
        for path in ('/mnt/guskit-rw', '/mnt/guskit-ro'):
            commands.extend([f'mkdir {path}', f'set_inode_field {path} mode 040755',
                             f'set_inode_field {path} uid 0', f'set_inode_field {path} gid 0'])
        for path, (content, mode) in files.items():
            source = tmp / Path(path).name
            source.touch(mode=0o600)
            source.write_text(content)
            commands.extend([f'rm {path}', f'write {source} {path}',
                             f'set_inode_field {path} mode 010{mode}',
                             f'set_inode_field {path} uid 0', f'set_inode_field {path} gid 0'])
        command_file = tmp / 'commands'
        command_file.write_text('\n'.join(commands) + '\n')
        subprocess.run([debugfs, '-w', '-f', str(command_file), str(image)], check=True, capture_output=True)
        for path, (content, _) in files.items():
            result = subprocess.run([debugfs, '-R', 'cat ' + path, str(image)], check=True, capture_output=True)
            if result.stdout != content.encode():
                raise ValueError('desktop staging readback differs: ' + path)



if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    parser.add_argument('suite')
    parser.add_argument('case')
    args = parser.parse_args()
    stage(args.image, args.suite, args.case)
