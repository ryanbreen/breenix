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
    servers = load_site()['SNTP_SERVERS'].split()
    if len(servers) > 8 or any(not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?', name) for name in servers):
        raise ValueError('SNTP_SERVERS needs up to eight DNS names or IPv4 addresses')
    debugfs, = require_e2fsprogs('debugfs')
    image = Path(image).resolve()
    with tempfile.TemporaryDirectory(prefix='qemu-desktop-') as tmp:
        tmp = Path(tmp)
        commands = []
        for name, content in (('desktop-suite', 'gui-apps\n'),
                              ('sntp-servers', ''.join(name + '\n' for name in servers))):
            source = tmp / name
            source.write_text(content)
            path = '/etc/breenix/' + name
            commands.extend([f'rm {path}', f'write {source} {path}',
                             f'set_inode_field {path} mode 0100644',
                             f'set_inode_field {path} uid 0', f'set_inode_field {path} gid 0'])
        command_file = tmp / 'commands'
        command_file.write_text('\n'.join(commands) + '\n')
        subprocess.run([debugfs, '-w', '-f', str(command_file), str(image)], check=True, capture_output=True)
        for name, content in (('desktop-suite', 'gui-apps\n'),
                              ('sntp-servers', ''.join(name + '\n' for name in servers))):
            result = subprocess.run([debugfs, '-R', 'cat /etc/breenix/' + name, str(image)], check=True, capture_output=True)
            if result.stdout != content.encode():
                raise ValueError('desktop staging readback differs: ' + name)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    parser.add_argument('suite')
    parser.add_argument('case')
    args = parser.parse_args()
    stage(args.image, args.suite, args.case)
