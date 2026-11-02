#!/usr/bin/env python3
"""Stage private site state in a disposable RAW-generation VM image."""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile

import vm_site


def stage(image, account):
    site = vm_site.load_site()
    if Path(account).read_text() != vm_site.metal_root.operator_account(site):
        raise ValueError('operator account does not match the VM site')
    user, uid = site['OPERATOR_USER'], site['OPERATOR_UID']
    if site['CONSOLE_USER'] != 'root':
        raise ValueError('VM server console requires the root account')
    if not vm_site.metal_root.SHA512_CRYPT.fullmatch(site['ROOT_PASSWORD_HASH']):
        raise ValueError('VM site needs a SHA-512-crypt root password hash')
    sources = site['SNTP_SERVERS'].split()
    if len(sources) > 8 or any(not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?', s) for s in sources):
        raise ValueError('invalid VM SNTP sources')

    def debug(command, write=False):
        result = subprocess.run(['debugfs'] + (['-w'] if write else []) + ['-R', command, image],
                                capture_output=True, text=True, check=True)
        return result.stdout

    passwd = debug('cat /etc/passwd')
    if any(line.split(':')[0] != user and line.split(':')[2:3] == [uid] for line in passwd.splitlines()):
        raise ValueError('VM operator uid is already in use')
    shadow = debug('cat /etc/shadow')
    root = 'root:' + site['ROOT_PASSWORD_HASH'] + ':::::::'
    shadow = '\n'.join([line for line in shadow.splitlines() if not line.startswith('root:')] + [root]) + '\n'
    files = {
        '/.breenix-shared/etc/passwd': (f'{user}:x:{uid}:{uid}:{user}:/home/{user}:/bin/bsh\n', 0o644),
        '/.breenix-shared/etc/group': (f'{user}:x:{uid}:\nwheel:x:10:{user}\n', 0o644),
        '/.breenix-shared/etc/shadow': (f'{user}:{site["OPERATOR_PASSWORD_HASH"]}:::::::\n', 0o600),
        '/.breenix-shared/var/lib/accounts/pending/' + user: (site['OPERATOR_SSH_PUBLIC_KEY'] + '\n', 0o600),
        '/etc/shadow': (shadow, 0o600),
        '/etc/breenix/sntp-servers': ('\n'.join(sources) + '\n', 0o644),
    }
    directories = [('/.breenix-shared', 0o700), ('/.breenix-shared/etc', 0o755),
                   ('/.breenix-shared/var', 0o755), ('/.breenix-shared/var/lib', 0o755),
                   ('/.breenix-shared/var/lib/accounts', 0o700),
                   ('/.breenix-shared/var/lib/accounts/pending', 0o700)]
    for path, mode in directories:
        debug('mkdir ' + path, True)
        debug(f'set_inode_field {path} mode 0{0o40000 | mode:o}', True)
        debug(f'set_inode_field {path} uid 0', True)
        debug(f'set_inode_field {path} gid 0', True)
    with tempfile.TemporaryDirectory(prefix='vm-site-', dir=Path(image).parent) as scratch:
        for index, (path, (content, mode)) in enumerate(files.items()):
            source = Path(scratch) / str(index)
            fd = os.open(source, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, 'w') as output:
                output.write(content)
            debug('rm ' + path, True)
            debug(f'write {source} {path}', True)
            debug(f'set_inode_field {path} mode 0{0o100000 | mode:o}', True)
            debug(f'set_inode_field {path} uid 0', True)
            debug(f'set_inode_field {path} gid 0', True)
            if debug('cat ' + path) != content:
                raise ValueError('VM site image readback failed: ' + path)


if __name__ == '__main__':
    stage(*sys.argv[1:])
